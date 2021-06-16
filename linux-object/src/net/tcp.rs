// Tcpsocket
#![allow(dead_code)]
// crate
use crate::net::get_ephemeral_port;
use crate::net::poll_ifaces;
use crate::net::IpEndpoint;
use crate::net::Endpoint;
use crate::net::SOCKETS;
use crate::net::TCP_SENDBUF;
use crate::net::TCP_RECVBUF;
use crate::net::GlobalSocketHandle;
use crate::error::LxError;
use crate::error::LxResult;
use crate::fs::FileLike;

// alloc
use alloc::boxed::Box;
use alloc::vec;

// smoltcp

use smoltcp::socket::TcpSocket;
use smoltcp::socket::TcpSocketBuffer;
use smoltcp::socket::TcpState;

// async
use async_trait::async_trait;

// third part
use rcore_fs::vfs::PollStatus;
use zircon_object::object::*;



/// missing documentation
pub struct TcpSocketState {
    /// missing documentation
    base: KObjectBase,
    /// missing documentation
    handle: GlobalSocketHandle,
    /// missing documentation
    local_endpoint: Option<IpEndpoint>, // save local endpoint for bind()
    /// missing documentation
    is_listening: bool,
}

impl TcpSocketState {
    /// missing documentation
    pub fn new() -> Self {
        let rx_buffer = TcpSocketBuffer::new(vec![0; TCP_RECVBUF]);
        let tx_buffer = TcpSocketBuffer::new(vec![0; TCP_SENDBUF]);
        let socket = TcpSocket::new(rx_buffer, tx_buffer);
        let handle = GlobalSocketHandle(SOCKETS.lock().add(socket));

        TcpSocketState {
            base: KObjectBase::new(),
            handle,
            local_endpoint: None,
            is_listening: false,
        }
    }

    /// missing documentation
    pub fn tcp_read(&self, data: &mut [u8]) -> (LxResult<usize>, Endpoint) {
        loop {
            let mut sockets = SOCKETS.lock();
            let mut socket = sockets.get::<TcpSocket>(self.handle.0);
            if socket.may_recv() {
                if let Ok(size) = socket.recv_slice(data) {
                    if size > 0 {
                        let endpoint = socket.remote_endpoint();
                        // avoid deadlock
                        drop(socket);
                        drop(sockets);

                        poll_ifaces();
                        return (Ok(size), Endpoint::Ip(endpoint));
                    }
                }
            } else {
                return (
                    Err(LxError::ENOTCONN),
                    Endpoint::Ip(IpEndpoint::UNSPECIFIED),
                );
            }
        }
    }

    /// missing documentation
    pub fn tcp_write(&self, data: &[u8], _sendto_endpoint: Option<Endpoint>) -> LxResult<usize> {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);
        if socket.is_open() {
            if socket.can_send() {
                match socket.send_slice(&data) {
                    Ok(size) => {
                        // avoid deadlock
                        drop(socket);
                        drop(sockets);

                        poll_ifaces();
                        Ok(size)
                    }
                    Err(_) => Err(LxError::ENOBUFS),
                }
            } else {
                Err(LxError::ENOBUFS)
            }
        } else {
            Err(LxError::ENOTCONN)
        }
    }

    /// missing documentation
    fn poll(&self) -> (bool, bool, bool) {
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get::<TcpSocket>(self.handle.0);

        let (mut input, mut output, mut err) = (false, false, false);
        if self.is_listening && socket.is_active() {
            // a new connection
            input = true;
        } else if !socket.is_open() {
            err = true;
        } else {
            if socket.can_recv() {
                input = true;
            }
            if socket.can_send() {
                output = true;
            }
        }
        (input, output, err)
    }

    /// missing documentation
    pub fn connect(&self, endpoint: Endpoint) -> LxResult<usize> {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);
        #[allow(warnings)]
        if let Endpoint::Ip(ip) = endpoint {
            let temp_port = get_ephemeral_port();

            match socket.connect(ip, temp_port) {
                Ok(()) => {
                    // avoid deadlock
                    drop(socket);
                    drop(sockets);

                    // wait for connection result
                    loop {
                        poll_ifaces();

                        let mut sockets = SOCKETS.lock();
                        let socket = sockets.get::<TcpSocket>(self.handle.0);
                        match socket.state() {
                            TcpState::SynSent => {
                                // still connecting
                                drop(socket);
                                debug!("poll for connection wait");
                                // SOCKET_ACTIVITY.wait(sockets);
                            }
                            TcpState::Established => {
                                break Ok(0);
                            }
                            _ => {
                                break Err(LxError::ECONNREFUSED);
                            }
                        }
                    }
                }
                Err(_) => Err(LxError::ENOBUFS),
            }
        } else {
            Err(LxError::EINVAL)
        }
    }

    /// missing documentation
    fn bind(&mut self, endpoint: Endpoint) -> LxResult<usize> {
        #[allow(warnings)]
        if let Endpoint::Ip(mut ip) = endpoint {
            if ip.port == 0 {
                ip.port = get_ephemeral_port();
            }
            self.local_endpoint = Some(ip);
            self.is_listening = false;
            Ok(0)
        } else {
            Err(LxError::EINVAL)
        }
    }

    /// missing documentation
    fn listen(&mut self) -> LxResult<usize> {
        if self.is_listening {
            // it is ok to listen twice
            return Ok(0);
        }
        let local_endpoint = self.local_endpoint.ok_or(LxError::EINVAL)?;
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);

        info!("socket listening on {:?}", local_endpoint);
        if socket.is_listening() {
            return Ok(0);
        }
        match socket.listen(local_endpoint) {
            Ok(()) => {
                self.is_listening = true;
                Ok(0)
            }
            Err(_) => Err(LxError::EINVAL),
        }
    }

    /// missing documentation
    fn shutdown(&self) -> LxResult<usize> {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);
        socket.close();
        Ok(0)
    }

    /// missing documentation
    fn accept(&mut self) -> Result<(Box<dyn FileLike>, Endpoint), LxError> {
        let endpoint = self.local_endpoint.ok_or(LxError::EINVAL)?;
        loop {
            let mut sockets = SOCKETS.lock();
            let socket = sockets.get::<TcpSocket>(self.handle.0);

            if socket.is_active() {
                let remote_endpoint = socket.remote_endpoint();
                drop(socket);

                let new_socket = {
                    let rx_buffer = TcpSocketBuffer::new(vec![0; TCP_RECVBUF]);
                    let tx_buffer = TcpSocketBuffer::new(vec![0; TCP_SENDBUF]);
                    let mut socket = TcpSocket::new(rx_buffer, tx_buffer);
                    socket.listen(endpoint).unwrap();
                    let new_handle = GlobalSocketHandle(sockets.add(socket));
                    let old_handle = ::core::mem::replace(&mut self.handle, new_handle);
                    Box::new(TcpSocketState {
                        base: KObjectBase::new(),
                        handle: old_handle,
                        local_endpoint: self.local_endpoint,
                        is_listening: false,
                    })
                };

                drop(sockets);
                poll_ifaces();
                return Ok((new_socket, Endpoint::Ip(remote_endpoint)));
            }

            drop(socket);
            // SOCKET_ACTIVITY.wait(sockets);
        }
    }

    /// missing documentation
    fn endpoint(&self) -> Option<Endpoint> {
        self.local_endpoint
            .clone()
            .map(|e| Endpoint::Ip(e))
            .or_else(|| {
                let mut sockets = SOCKETS.lock();
                let socket = sockets.get::<TcpSocket>(self.handle.0);
                let endpoint = socket.local_endpoint();
                if endpoint.port != 0 {
                    Some(Endpoint::Ip(endpoint))
                } else {
                    None
                }
            })
    }

    /// missing documentation
    fn remote_endpoint(&self) -> Option<Endpoint> {
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get::<TcpSocket>(self.handle.0);
        if socket.is_open() {
            Some(Endpoint::Ip(socket.remote_endpoint()))
        } else {
            None
        }
    }

    // fn box_clone(&self) -> Box<dyn FileLike> {
    //     Box::new(self.clone())
    // }

    fn tcp_ioctl(&self) -> LxResult<usize> {
        Err(LxError::ENOSYS)
    }
}
impl_kobject!(TcpSocketState);

#[async_trait]
impl FileLike for TcpSocketState {
    /// read to buffer
    async fn read(&self, _buf: &mut [u8]) -> LxResult<usize> {
        let (a,_b) = self.tcp_read(_buf);
        a
        // unimplemented!()
    }
    /// write from buffer
    fn write(&self, _buf: &[u8]) -> LxResult<usize> {
        self.tcp_write(_buf, None)
        // unimplemented!()
    }
    /// read to buffer at given offset
    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
        unimplemented!()
    }
    /// write from buffer at given offset
    fn write_at(&self, _offset: u64, _buf: &[u8]) -> LxResult<usize> {
        unimplemented!()
    }
    /// wait for some event on a file descriptor
    fn poll(&self) -> LxResult<PollStatus> {
        self.poll();
        unimplemented!()
    }
    /// wait for some event on a file descriptor use async
    async fn async_poll(&self) -> LxResult<PollStatus> {
        unimplemented!()
    }
    /// manipulates the underlying device parameters of special files
    fn ioctl(&self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> LxResult<usize> {
        self.tcp_ioctl()
    }
    /// manipulate file descriptor
    fn fcntl(&self, _cmd: usize, _arg: usize) -> LxResult<usize> {
        // unimplemented!()
        Ok(0)
    }
}
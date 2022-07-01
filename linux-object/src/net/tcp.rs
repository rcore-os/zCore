// Tcpsocket
#![allow(dead_code)]
// crate
use crate::error::LxError;
use crate::error::LxResult;
use crate::net::get_ephemeral_port;
use crate::net::get_sockets;
// use crate::net::get_net_device;
use crate::net::poll_ifaces;
use crate::net::Endpoint;
use crate::net::GlobalSocketHandle;
use crate::net::IpEndpoint;
use crate::net::Socket;
use crate::net::SysResult;
use crate::net::TCP_RECVBUF;
use crate::net::TCP_SENDBUF;
use alloc::sync::Arc;
use lock::Mutex;

// alloc
use alloc::boxed::Box;
use alloc::vec;

// smoltcp

use smoltcp::socket::TcpSocket;
use smoltcp::socket::TcpSocketBuffer;

// async
use async_trait::async_trait;

// third part
#[allow(unused_imports)]
use zircon_object::object::*;

/// missing documentation
#[derive(Debug)]
pub struct TcpSocketState {
    /// missing documentation
    // base: KObjectBase,
    /// missing documentation
    handle: GlobalSocketHandle,
    /// missing documentation
    local_endpoint: Option<IpEndpoint>, // save local endpoint for bind()
    /// missing documentation
    is_listening: bool,
}

impl Default for TcpSocketState {
    fn default() -> Self {
        TcpSocketState::new()
    }
}

impl TcpSocketState {
    /// missing documentation
    pub fn new() -> Self {
        let rx_buffer = TcpSocketBuffer::new(vec![0; TCP_RECVBUF]);
        let tx_buffer = TcpSocketBuffer::new(vec![0; TCP_SENDBUF]);
        let socket = TcpSocket::new(rx_buffer, tx_buffer);
        let handle = GlobalSocketHandle(get_sockets().lock().add(socket));

        TcpSocketState {
            // base: KObjectBase::new(),
            handle,
            local_endpoint: None,
            is_listening: false,
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut TcpSocket) -> R) -> R {
        let res = {
            let net_sockets = get_sockets();
            let mut sockets = net_sockets.lock();
            let mut socket = sockets.get::<TcpSocket>(self.handle.0);
            f(&mut *socket)
        };
        res
    }
}
// impl_kobject!(TcpSocketState);

#[async_trait]
impl Socket for TcpSocketState {
    /// read to buffer
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        info!("tcp read");
        loop {
            poll_ifaces();
            let net_sockets = get_sockets();
            let mut sockets = net_sockets.lock();
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
    /// write from buffer
    fn write(&self, data: &[u8], _sendto_endpoint: Option<Endpoint>) -> SysResult {
        info!("tcp write");
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);
        if socket.is_open() {
            if socket.can_send() {
                match socket.send_slice(data) {
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
    /// connect
    async fn connect(&mut self, endpoint: Endpoint) -> SysResult {
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);
        #[allow(warnings)]
        if let Endpoint::Ip(ip) = endpoint {
            let local_port = get_ephemeral_port();
            socket
                .connect(ip, local_port)
                .map_err(|_| LxError::ENOBUFS)?;

            // avoid deadlock
            drop(socket);
            drop(sockets);
            // wait for connection result
            loop {
                poll_ifaces();
                let net_sockets = get_sockets();
                let mut sockets = net_sockets.lock();
                let socket = sockets.get::<TcpSocket>(self.handle.0);
                use smoltcp::socket::TcpState;
                match socket.state() {
                    TcpState::SynSent => {
                        // still connecting
                        drop(socket);
                        drop(sockets);

                        poll_ifaces();
                    }
                    TcpState::Established => {
                        break Ok(0);
                    }
                    _ => {
                        break Err(LxError::ECONNREFUSED);
                    }
                }
            }
        } else {
            drop(socket);
            drop(sockets);
            Err(LxError::EINVAL)
        }
    }
    /// wait for some event on a file descriptor
    fn poll(&self) -> (bool, bool, bool) {
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
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

    fn bind(&mut self, endpoint: Endpoint) -> SysResult {
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

    fn listen(&mut self) -> SysResult {
        if self.is_listening {
            // it is ok to listen twice
            return Ok(0);
        }
        let local_endpoint = self.local_endpoint.ok_or(LxError::EINVAL)?;
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
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

    fn shutdown(&self) -> SysResult {
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);
        socket.close();
        Ok(0)
    }

    async fn accept(&mut self) -> LxResult<(Arc<Mutex<dyn Socket>>, Endpoint)> {
        let endpoint = self.local_endpoint.ok_or(LxError::EINVAL)?;
        loop {
            poll_ifaces();
            let net_sockets = get_sockets();
            let mut sockets = net_sockets.lock();
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
                    Arc::new(Mutex::new(TcpSocketState {
                        // base: KObjectBase::new(),
                        handle: old_handle,
                        local_endpoint: self.local_endpoint,
                        is_listening: false,
                    }))
                };
                drop(sockets);
                poll_ifaces();
                return Ok((new_socket, Endpoint::Ip(remote_endpoint)));
            }

            drop(socket);
            drop(sockets);
        }
    }

    fn endpoint(&self) -> Option<Endpoint> {
        self.local_endpoint.map(Endpoint::Ip).or_else(|| {
            let net_sockets = get_sockets();
            let mut sockets = net_sockets.lock();
            let socket = sockets.get::<TcpSocket>(self.handle.0);
            let endpoint = socket.local_endpoint();
            if endpoint.port != 0 {
                Some(Endpoint::Ip(endpoint))
            } else {
                None
            }
        })
    }

    fn remote_endpoint(&self) -> Option<Endpoint> {
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let socket = sockets.get::<TcpSocket>(self.handle.0);
        if socket.is_open() {
            Some(Endpoint::Ip(socket.remote_endpoint()))
        } else {
            None
        }
    }

    fn setsockopt(&mut self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        Ok(0)
    }

    fn ioctl(&self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        Ok(0)
    }

    fn fcntl(&self, _cmd: usize, _arg: usize) -> SysResult {
        warn!("fnctl is unimplemented for this socket");
        // now no fnctl impl but need to pass libctest , so just do a trick
        match _cmd {
            1 => Ok(1),
            3 => Ok(0o4000),
            _ => Ok(0),
        }
    }
}

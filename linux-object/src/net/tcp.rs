// Tcpsocket

// crate
use crate::error::LxError;
use crate::error::LxResult;
use crate::fs::{FileLike, OpenFlags, PollStatus};
use crate::net::*;
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

/// TCP socket structure
pub struct TcpSocketState {
    /// Kernel object base
    base: KObjectBase,
    /// TcpSocket Inner
    inner: Mutex<TcpInner>,
}

/// TCP socket inner
pub struct TcpInner {
    /// missing documentation
    handle: GlobalSocketHandle,
    /// missing documentation
    local_endpoint: Option<IpEndpoint>, // save local endpoint for bind()
    /// missing documentation
    is_listening: bool,
    /// flags on the socket
    flags: OpenFlags,
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
            base: KObjectBase::new(),
            inner: Mutex::new(TcpInner {
                handle,
                local_endpoint: None,
                is_listening: false,
                flags: OpenFlags::empty(),
            }),
        }
    }
}

#[async_trait]
impl Socket for TcpSocketState {
    /// read to buffer
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        info!("tcp read");
        loop {
            poll_ifaces();
            let net_sockets = get_sockets();
            let mut sockets = net_sockets.lock();
            let mut socket = sockets.get::<TcpSocket>(self.inner.lock().handle.0);
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
        let mut socket = sockets.get::<TcpSocket>(self.inner.lock().handle.0);
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
    async fn connect(&self, endpoint: Endpoint) -> SysResult {
        let inner = self.inner.lock();
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let mut socket = sockets.get::<TcpSocket>(inner.handle.0);
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
                let socket = sockets.get::<TcpSocket>(inner.handle.0);
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
        let inner = self.inner.lock();
        let sockets = get_sockets();
        let mut set = sockets.lock();
        let socket = set.get::<TcpSocket>(inner.handle.0);

        let (mut read, mut write, mut error) = (false, false, false);
        if inner.is_listening && socket.is_active() {
            // a new connection
            read = true;
        } else if !socket.is_open() {
            error = true;
        } else {
            if socket.can_recv() {
                read = true; // POLLIN
            }
            if socket.can_send() {
                write = true; // POLLOUT
            }
        }
        (read, write, error)
    }

    fn bind(&self, endpoint: Endpoint) -> SysResult {
        let mut inner = self.inner.lock();
        if let Endpoint::Ip(mut ip) = endpoint {
            if ip.port == 0 {
                ip.port = get_ephemeral_port();
            }
            inner.local_endpoint = Some(ip);
            inner.is_listening = false;
            Ok(0)
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn listen(&self) -> SysResult {
        let mut inner = self.inner.lock();
        if inner.is_listening {
            // it is ok to listen twice
            return Ok(0);
        }
        let local_endpoint = inner.local_endpoint.ok_or(LxError::EINVAL)?;
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let mut socket = sockets.get::<TcpSocket>(inner.handle.0);

        info!("socket listening on {:?}", local_endpoint);
        if socket.is_listening() {
            return Ok(0);
        }
        match socket.listen(local_endpoint) {
            Ok(()) => {
                inner.is_listening = true;
                Ok(0)
            }
            Err(_) => Err(LxError::EINVAL),
        }
    }

    fn shutdown(&self) -> SysResult {
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let mut socket = sockets.get::<TcpSocket>(self.inner.lock().handle.0);
        socket.close();
        Ok(0)
    }

    async fn accept(&self) -> LxResult<(Arc<dyn FileLike>, Endpoint)> {
        let mut inner = self.inner.lock();
        let endpoint = inner.local_endpoint.ok_or(LxError::EINVAL)?;
        loop {
            poll_ifaces();
            let net_sockets = get_sockets();
            let mut sockets = net_sockets.lock();
            let socket = sockets.get::<TcpSocket>(inner.handle.0);
            if socket.is_active() {
                let remote_endpoint = socket.remote_endpoint();
                drop(socket);
                let new_socket = {
                    let rx_buffer = TcpSocketBuffer::new(vec![0; TCP_RECVBUF]);
                    let tx_buffer = TcpSocketBuffer::new(vec![0; TCP_SENDBUF]);
                    let mut socket = TcpSocket::new(rx_buffer, tx_buffer);
                    socket.listen(endpoint).unwrap();
                    let new_handle = GlobalSocketHandle(sockets.add(socket));
                    let old_handle = ::core::mem::replace(&mut inner.handle, new_handle);

                    Arc::new(TcpSocketState {
                        base: KObjectBase::new(),
                        inner: Mutex::new(TcpInner {
                            handle: old_handle,
                            local_endpoint: inner.local_endpoint,
                            is_listening: false,
                            flags: OpenFlags::empty(), // TODO, Get flags from args
                        }),
                    })
                };
                drop(sockets);
                poll_ifaces();
                return Ok((
                    new_socket as Arc<dyn FileLike>,
                    Endpoint::Ip(remote_endpoint),
                ));
            }

            drop(socket);
            drop(sockets);
        }
    }

    fn endpoint(&self) -> Option<Endpoint> {
        let inner = self.inner.lock();
        inner.local_endpoint.map(Endpoint::Ip).or_else(|| {
            let net_sockets = get_sockets();
            let mut sockets = net_sockets.lock();
            let socket = sockets.get::<TcpSocket>(inner.handle.0);
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
        let socket = sockets.get::<TcpSocket>(self.inner.lock().handle.0);
        if socket.is_open() {
            Some(Endpoint::Ip(socket.remote_endpoint()))
        } else {
            None
        }
    }
}

impl_kobject!(TcpSocketState);

#[async_trait]
impl FileLike for TcpSocketState {
    fn flags(&self) -> OpenFlags {
        self.inner.lock().flags
    }

    fn set_flags(&self, f: OpenFlags) -> LxResult {
        let flags = &mut self.inner.lock().flags;

        // See fcntl, only O_APPEND, O_ASYNC, O_DIRECT, O_NOATIME, O_NONBLOCK
        flags.set(OpenFlags::APPEND, f.contains(OpenFlags::APPEND));
        flags.set(OpenFlags::NON_BLOCK, f.contains(OpenFlags::NON_BLOCK));
        flags.set(OpenFlags::CLOEXEC, f.contains(OpenFlags::CLOEXEC));
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        unimplemented!()
        /*
        let sockets = get_sockets();
        let mut set = sockets.lock();
        let socket = set.get::<TcpSocket>(self.handle.0);
        let new_handle = GlobalSocketHandle(set.add(*socket));
        Arc::new(Self {
            base: KObjectBase::new(),
            handle: new_handle,
            local_endpoint: self.local_endpoint,
            is_listening: self.is_listening,
            flags: RwLock::new(*self.flags.read()),
        })
        */
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        Socket::read(self, buf).await.0
    }

    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
        unimplemented!()
    }

    fn write(&self, buf: &[u8]) -> LxResult<usize> {
        Socket::write(self, buf, None)
    }

    fn poll(&self) -> LxResult<PollStatus> {
        let (read, write, error) = Socket::poll(self);
        Ok(PollStatus { read, write, error })
    }

    async fn async_poll(&self) -> LxResult<PollStatus> {
        unimplemented!()
    }

    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize> {
        Socket::ioctl(self, request, arg1, arg2, arg3)
    }

    fn as_socket(&self) -> LxResult<&dyn Socket> {
        Ok(self)
    }
}

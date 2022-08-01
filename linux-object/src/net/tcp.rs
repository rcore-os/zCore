// Tcpsocket

// crate
use crate::error::{LxError, LxResult};
use crate::fs::{FileLike, OpenFlags, PollStatus};
use crate::net::*;
use alloc::sync::Arc;
use lock::Mutex;

// alloc
use alloc::boxed::Box;
use alloc::vec;

// smoltcp
use smoltcp::socket::{TcpSocket, TcpSocketBuffer, TcpState};

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
                flags: OpenFlags::RDWR,
            }),
        }
    }
}

#[async_trait]
impl Socket for TcpSocketState {
    /// read to buffer
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        info!("tcp read");
        let inner = self.inner.lock();
        loop {
            //poll_ifaces();

            let sets = get_sockets();
            let mut sets = sets.lock();
            let mut socket = sets.get::<TcpSocket>(inner.handle.0);

            let copied_len = socket.recv_slice(data);
            // avoid deadlock in poll_ifaces()
            drop(socket);
            drop(sets);

            match copied_len {
                Ok(0) | Err(smoltcp::Error::Exhausted) => {
                    poll_ifaces();
                    if inner.flags.contains(OpenFlags::NON_BLOCK) {
                        return (Err(LxError::EAGAIN), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                    } else {
                        // Continue reading
                        trace!("Continue reading");
                    }
                }
                Ok(size) => {
                    let endpoint = get_sockets()
                        .lock()
                        .get::<TcpSocket>(inner.handle.0)
                        .remote_endpoint();
                    return (Ok(size), Endpoint::Ip(endpoint));
                }
                Err(err) => {
                    error!("Tcp socket read error: {:?}", err);
                    return (
                        Err(LxError::ENOTCONN),
                        Endpoint::Ip(IpEndpoint::UNSPECIFIED),
                    );
                }
            }
        }
    }
    /// write from buffer
    fn write(&self, data: &[u8], _sendto_endpoint: Option<Endpoint>) -> SysResult {
        //loop {
        let sets = get_sockets();
        let mut sets = sets.lock();
        let mut socket = sets.get::<TcpSocket>(self.inner.lock().handle.0);
        let copied_len = socket.send_slice(data);

        drop(socket);
        drop(sets);
        poll_ifaces();

        match copied_len {
            Ok(size) => Ok(size),
            Err(err) => {
                error!("Tcp socket write error: {:?}", err);
                Err(LxError::ENOBUFS)
            }
        }
        //}
    }
    /// connect
    async fn connect(&self, endpoint: Endpoint) -> SysResult {
        let inner = self.inner.lock();
        #[allow(warnings)]
        if let Endpoint::Ip(ip) = endpoint {
            get_sockets()
                .lock()
                .get::<TcpSocket>(inner.handle.0)
                .connect(ip, get_ephemeral_port())
                .map_err(|_| LxError::ENOBUFS)?;

            let mut tc = 0;
            // wait for connection result
            loop {
                //Submit a SYN
                poll_ifaces();

                match get_sockets()
                    .lock()
                    .get::<TcpSocket>(inner.handle.0)
                    .state()
                {
                    TcpState::SynSent => {
                        // still connecting
                    }
                    TcpState::Established => {
                        return Ok(0);
                    }
                    _ => {
                        warn!("connect failed ...");
                        if tc < 10 {
                            tc += 1;
                        } else {
                            return Err(LxError::ECONNREFUSED);
                        }
                    }
                }
            }
        } else {
            error!("connect: bad endpoint");
            Err(LxError::EINVAL)
        }
    }
    /// wait for some event on a file descriptor
    fn poll(&self, events: PollEvents) -> (bool, bool, bool) {
        //poll_ifaces();
        let inner = self.inner.lock();
        let (recv_state, send_state) = {
            let sets = get_sockets();
            let mut sets = sets.lock();
            let socket = sets.get::<TcpSocket>(inner.handle.0);
            debug!(
                "tcp is_listening: {:?}, now tcp state: {:?}",
                inner.is_listening,
                socket.state()
            );

            (socket.can_recv(), socket.can_send())
        };
        if (events.contains(PollEvents::IN) && !recv_state)
            || (events.contains(PollEvents::OUT) && !send_state)
        {
            poll_ifaces();
        }

        let (mut read, mut write, mut error) = (false, false, false);

        let sets = get_sockets();
        let mut sets = sets.lock();
        let socket = sets.get::<TcpSocket>(inner.handle.0);

        //Todo, syscall async poll needs to be executed after first Pending
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
        debug!("tcp poll: {:?}", (read, write, error));
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
            info!("It's already listening");
            return Ok(0);
        }

        let local_endpoint = inner.local_endpoint.ok_or(LxError::EINVAL)?;
        info!("socket listening on {:?}", local_endpoint);

        let sets = get_sockets();
        let mut sets = sets.lock();
        let mut socket = sets.get::<TcpSocket>(inner.handle.0);

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
        let sets = get_sockets();
        let mut sets = sets.lock();
        let mut socket = sets.get::<TcpSocket>(self.inner.lock().handle.0);
        socket.close();
        Ok(0)
    }

    async fn accept(&self) -> LxResult<(Arc<dyn FileLike>, Endpoint)> {
        let mut inner = self.inner.lock();
        let endpoint = inner.local_endpoint.ok_or(LxError::EINVAL)?;
        loop {
            //poll_ifaces();
            let sets = get_sockets();
            let mut sets = sets.lock();
            let socket = sets.get::<TcpSocket>(inner.handle.0);
            if socket.is_active() {
                let remote_endpoint = socket.remote_endpoint();
                drop(socket);
                drop(sets);

                let new_socket = {
                    let rx_buffer = TcpSocketBuffer::new(vec![0; TCP_RECVBUF]);
                    let tx_buffer = TcpSocketBuffer::new(vec![0; TCP_SENDBUF]);
                    let mut socket = TcpSocket::new(rx_buffer, tx_buffer);
                    socket.listen(endpoint).unwrap();

                    let new_handle = GlobalSocketHandle(get_sockets().lock().add(socket));
                    let old_handle = ::core::mem::replace(&mut inner.handle, new_handle);

                    Arc::new(TcpSocketState {
                        base: KObjectBase::new(),
                        inner: Mutex::new(TcpInner {
                            handle: old_handle,
                            local_endpoint: inner.local_endpoint,
                            is_listening: false,
                            flags: OpenFlags::RDWR,
                        }),
                    })
                };

                return Ok((
                    new_socket as Arc<dyn FileLike>,
                    Endpoint::Ip(remote_endpoint),
                ));
            } else {
                drop(socket);
                drop(sets);
                poll_ifaces();
            }
        }
    }

    fn endpoint(&self) -> Option<Endpoint> {
        let inner = self.inner.lock();
        inner.local_endpoint.map(Endpoint::Ip).or_else(|| {
            let sets = get_sockets();
            let mut sets = sets.lock();
            let socket = sets.get::<TcpSocket>(inner.handle.0);
            let endpoint = socket.local_endpoint();
            if endpoint.port != 0 {
                Some(Endpoint::Ip(endpoint))
            } else {
                None
            }
        })
    }

    fn remote_endpoint(&self) -> Option<Endpoint> {
        let sets = get_sockets();
        let mut sets = sets.lock();
        let socket = sets.get::<TcpSocket>(self.inner.lock().handle.0);
        if socket.is_open() {
            Some(Endpoint::Ip(socket.remote_endpoint()))
        } else {
            None
        }
    }

    fn get_buffer_capacity(&self) -> Option<(usize, usize)> {
        let sockets = get_sockets();
        let mut set = sockets.lock();
        let socket = set.get::<TcpSocket>(self.inner.lock().handle.0);
        let (recv_ca, send_ca) = (socket.recv_capacity(), socket.send_capacity());
        Some((recv_ca, send_ca))
    }

    fn socket_type(&self) -> Option<SocketType> {
        Some(SocketType::SOCK_STREAM)
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

    fn poll(&self, events: PollEvents) -> LxResult<PollStatus> {
        let (read, write, error) = Socket::poll(self, events);
        Ok(PollStatus { read, write, error })
    }

    async fn async_poll(&self, events: PollEvents) -> LxResult<PollStatus> {
        let (read, write, error) = Socket::poll(self, events);
        Ok(PollStatus { read, write, error })
    }

    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize> {
        Socket::ioctl(self, request, arg1, arg2, arg3)
    }

    fn as_socket(&self) -> LxResult<&dyn Socket> {
        Ok(self)
    }
}

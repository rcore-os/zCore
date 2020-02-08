use super::*;
use smoltcp::socket::{TcpSocket, TcpSocketBuffer, TcpState};

#[derive(Debug)]
pub struct TcpSocketState {
    inner: Mutex<TcpInner>,
}

struct TcpInner {
    handle: GlobalSocketHandle,
    local_endpoint: Option<IpEndpoint>, // save local endpoint for bind()
    is_listening: bool,
}

impl TcpSocketState {
    pub fn new() -> Self {
        let rx_buffer = TcpSocketBuffer::new(vec![0; TCP_RECVBUF]);
        let tx_buffer = TcpSocketBuffer::new(vec![0; TCP_SENDBUF]);
        let socket = TcpSocket::new(rx_buffer, tx_buffer);
        let handle = GlobalSocketHandle(SOCKETS.lock().add(socket));

        TcpSocketState {
            inner: Mutex::new(TcpInner {
                handle,
                local_endpoint: None,
                is_listening: false,
            })
        }
    }
}

#[async_trait]
impl Socket for TcpSocketState {
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        loop {
            poll_ifaces();
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

    fn write(&self, data: &[u8], _sendto_endpoint: Option<Endpoint>) -> SysResult {
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

    async fn connect(&self, endpoint: Endpoint) -> SysResult {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);

        if let Endpoint::Ip(ip) = endpoint {
            let temp_port = get_ephemeral_port();

            socket
                .connect(ip, temp_port)
                .map_err(|_| LxError::ENOBUFS)?;
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
                        drop(sockets);
                        IFaceFuture.await;
                    }
                    TcpState::Established => {
                        return Ok(0);
                    }
                    _ => {
                        return Err(LxError::ECONNREFUSED);
                    }
                }
            }
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn bind(&self, endpoint: Endpoint) -> SysResult {
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

    fn listen(&self) -> SysResult {
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

    fn shutdown(&self) -> SysResult {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);
        socket.close();
        Ok(0)
    }

    async fn accept(&self) -> LxResult<(Box<dyn Socket>, Endpoint)> {
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
                    let old_handle = core::mem::replace(&self.handle, new_handle);

                    Box::new(TcpSocketState {
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
            drop(sockets);
            IFaceFuture.await;
        }
    }

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

    fn remote_endpoint(&self) -> Option<Endpoint> {
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get::<TcpSocket>(self.handle.0);
        if socket.is_open() {
            Some(Endpoint::Ip(socket.remote_endpoint()))
        } else {
            None
        }
    }
}

pub const TCP_SENDBUF: usize = 512 * 1024; // 512K
pub const TCP_RECVBUF: usize = 512 * 1024; // 512K

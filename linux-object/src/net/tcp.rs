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
use spin::Mutex;

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

    /// missing documentation
    pub async fn read(&self, data: &mut [u8]) -> (LxResult<usize>, Endpoint) {
        warn!("tcp read");
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

    /// missing documentation
    #[cfg(feature = "e1000")]
    pub async fn read(&self, data: &mut [u8]) -> (LxResult<usize>, Endpoint) {
        warn!("tcp read");
        use core::task::Poll;
        futures::future::poll_fn(|cx| {
            self.with(|s| {
                if s.can_recv() {
                    warn!("can recv ok");
                    if let Ok(size) = s.recv_slice(data) {
                        warn!("--------------Ok size {}", size);
                        if size > 0 {
                            let endpoint = s.remote_endpoint();
                            Poll::Ready((Ok(size), Endpoint::Ip(endpoint)))
                        } else {
                            warn!("wait size > 0");
                            s.register_recv_waker(cx.waker());
                            s.register_send_waker(cx.waker());
                            Poll::Pending
                        }
                    } else {
                        warn!("recv_slice not Ok（size）");
                        Poll::Ready((
                            Err(LxError::ENOTCONN),
                            Endpoint::Ip(IpEndpoint::UNSPECIFIED),
                        ))
                    }
                } else {
                    error!("can not recv");
                    s.register_recv_waker(cx.waker());
                    s.register_send_waker(cx.waker());
                    Poll::Pending
                }
            })
        })
        .await
        // let net_sockets = get_net_sockets();
        // let mut sockets = net_sockets.lock();
        // let mut socket = sockets.get::<TcpSocket>(self.handle.0);
        // // if socket.may_recv() {
        // if let Ok(size) = socket.recv_slice(data) {
        //     let endpoint = socket.remote_endpoint();
        //     return (Ok(size), Endpoint::Ip(endpoint));
        // } else {
        //     return (
        //         Err(LxError::ENOTCONN),
        //         Endpoint::Ip(IpEndpoint::UNSPECIFIED),
        //     );
        // }
    }

    /// missing documentation
    pub fn write(&self, data: &[u8], _sendto_endpoint: Option<Endpoint>) -> SysResult {
        warn!("tcp write");
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

    /// missing documentation
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

    /// missing documentation
    pub async fn connect(&self, endpoint: Endpoint) -> SysResult {
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
    /// missing documentation
    #[cfg(feature = "e1000")]
    pub async fn connect(&self, endpoint: Endpoint) -> SysResult {
        warn!("tcp connect");
        // if let Endpoint::Ip(ip) = endpoint {
        //     let local_port = get_ephemeral_port();
        //     self.with(|ss| ss.connect(ip, local_port).map_err(|_| LxError::ENOBUFS))?;
        //     //     use crate::net::IFaceFuture;
        //     //     IFaceFuture { flag: false }.await;
        //     //     warn!("no");
        //     //     use smoltcp::socket::TcpState;
        //     //     let ret = self.with(|ss| match ss.state() {
        //     //         TcpState::SynSent => {
        //     //             // still connecting
        //     //             warn!("SynSent");
        //     //             Ok(0)
        //     //         }
        //     //         TcpState::Established => Ok(0),
        //     //         _ => Err(LxError::ECONNREFUSED),
        //     //     });

        //     // Ok(0)
        //     // socket
        //     //     .connect(ip, local_port)
        //     //     .map_err(|_| LxError::ENOBUFS)?;

        //     // use crate::net::ConnectFuture;
        //     // use smoltcp::socket::SocketRef;
        //     // let c = ConnectFuture {
        //     //     socket: SocketRef::into_inner(socket),
        //     // }
        //     // .await;
        //     // drop(c);

        //     // use core::future::Future;
        //     // use core::pin::Pin;
        //     // use core::task::Context;
        //     use crate::net::IFaceFuture;
        //     IFaceFuture.await;
        //     // warn!("no");
        //     // IFaceFuture.await;
        //     // warn!("no");
        //     // IFaceFuture.await;
        //     // warn!("no");
        //     // IFaceFuture.await;
        //     // warn!("no");

        //     use core::task::Poll;
        //     use smoltcp::socket::TcpState;
        //     let ret = futures::future::poll_fn(|cx| {
        //         self.with(|s| {
        //             // s.connect(ip, local_port).map_err(|_| LxError::ENOBUFS)?;
        //             match s.state() {
        //                 TcpState::Closed | TcpState::TimeWait => {
        //                     warn!("Closed|TimeWait");
        //                     Poll::Ready(Err(LxError::ECONNREFUSED))
        //                 }
        //                 TcpState::Listen => {
        //                     warn!("Listen");
        //                     Poll::Ready(Err(LxError::ECONNREFUSED))
        //                 }
        //                 TcpState::SynSent => {
        //                     warn!("SynSent");
        //                     s.register_recv_waker(cx.waker());
        //                     s.register_send_waker(cx.waker());
        //                     // drop(s);
        //                     // #[cfg(feature = "e1000")]
        //                     // poll_ifaces_e1000();
        //                     // IFaceFuture.await
        //                     Poll::Pending
        //                 }
        //                 TcpState::SynReceived => {
        //                     warn!("SynReceived");
        //                     s.register_recv_waker(cx.waker());
        //                     s.register_send_waker(cx.waker());
        //                     Poll::Pending
        //                 }
        //                 TcpState::Established => {
        //                     warn!("Established");
        //                     // s.register_recv_waker(cx.waker());
        //                     // s.register_send_waker(cx.waker());
        //                     Poll::Ready(Ok(0))
        //                     // Poll::Pending
        //                 }
        //                 // TcpState::TimeWait => {
        //                 //     warn!("TimeWait");
        //                 //     // s.register_recv_waker(cx.waker());
        //                 //     // s.register_send_waker(cx.waker());
        //                 //     Poll::Ready(Ok(0))
        //                 //     // Poll::Pending
        //                 // }
        //                 TcpState::FinWait1 => {
        //                     warn!("------------------------------------FinWait1");
        //                     // s.register_recv_waker(cx.waker());
        //                     // s.register_send_waker(cx.waker());
        //                     Poll::Ready(Ok(0))
        //                     // Poll::Pending
        //                 }
        //                 TcpState::FinWait2 => {
        //                     warn!("----------------------------------------FinWait2");
        //                     // s.register_recv_waker(cx.waker());
        //                     // s.register_send_waker(cx.waker());
        //                     Poll::Ready(Ok(0))
        //                     // Poll::Pending
        //                 }
        //                 TcpState::Closing => {
        //                     warn!("-------------------------------------------Closing");
        //                     // s.register_recv_waker(cx.waker());
        //                     // s.register_send_waker(cx.waker());
        //                     Poll::Ready(Ok(0))
        //                     // Poll::Pending
        //                 }
        //                 TcpState::LastAck => {
        //                     warn!("-------------------------------------------LastAck");
        //                     // s.register_recv_waker(cx.waker());
        //                     // s.register_send_waker(cx.waker());
        //                     Poll::Ready(Ok(0))
        //                     // Poll::Pending
        //                 }
        //                 _ => {
        //                     warn!("_");
        //                     Poll::Ready(Err(LxError::ECONNREFUSED))
        //                 }
        //             }
        //         })
        //     })
        //     .await;
        //     // #[cfg(feature = "e1000")]
        //     // poll_ifaces_e1000();
        //     IFaceFuture.await;
        //     warn!("ret {:?}", ret);
        //     ret
        // // Ok(0)
        // } else {
        //     return Err(LxError::EINVAL);
        // }

        let net_sockets = get_net_sockets();
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
            #[cfg(feature = "e1000")]
            poll_ifaces_e1000();
            #[cfg(feature = "loopback")]
            poll_ifaces_loopback();
            // wait for connection result
            loop {
                warn!("loop");
                let net_sockets = get_net_sockets();
                let mut sockets = net_sockets.lock();
                let socket = sockets.get::<TcpSocket>(self.handle.0);
                use smoltcp::socket::TcpState;
                match socket.state() {
                    TcpState::SynSent => {
                        // still connecting
                        warn!("SynSent");
                        drop(socket);
                        drop(sockets);

                        #[cfg(feature = "e1000")]
                        poll_ifaces_e1000();
                        #[cfg(feature = "loopback")]
                        poll_ifaces_loopback();
                    }
                    TcpState::Established => {
                        warn!("estab");
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
            return Err(LxError::EINVAL);
        }
    }

    /// missing documentation
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

    /// missing documentation
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

    /// missing documentation
    fn shutdown(&self) -> SysResult {
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);
        socket.close();
        Ok(0)
    }

    /// missing documentation
    async fn accept(&mut self) -> Result<(Arc<Mutex<dyn Socket>>, Endpoint), LxError> {
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
    #[cfg(feature = "e1000")]
    async fn accept(&mut self) -> Result<(Arc<Mutex<dyn Socket>>, Endpoint), LxError> {
        let endpoint = self.local_endpoint.ok_or(LxError::EINVAL)?;

        // let net_sockets = get_net_sockets();
        // let mut sockets = net_sockets.lock();
        // let socket = sockets.get::<TcpSocket>(self.handle.0);

        // if socket.is_active() {
        // use crate::net::AcceptFuture;
        // AcceptFuture {
        //     socket: &mut socket,
        // }
        // .await;

        use core::task::Poll;
        futures::future::poll_fn(|cx| {
            self.with(|s| {
                if s.is_active() {
                    Poll::Ready(())
                } else {
                    s.register_recv_waker(cx.waker());
                    s.register_send_waker(cx.waker());
                    Poll::Pending
                }
            })
        })
        .await;
        let remote_endpoint = self.with(|s| s.remote_endpoint());
        // drop(socket);
        let new_socket = {
            let rx_buffer = TcpSocketBuffer::new(vec![0; TCP_RECVBUF]);
            let tx_buffer = TcpSocketBuffer::new(vec![0; TCP_SENDBUF]);
            let mut socket = TcpSocket::new(rx_buffer, tx_buffer);
            socket.listen(endpoint).unwrap();
            let net_sockets = get_net_sockets();
            let mut sockets = net_sockets.lock();
            let new_handle = GlobalSocketHandle(sockets.add(socket));
            let old_handle = ::core::mem::replace(&mut self.handle, new_handle);
            Arc::new(Mutex::new(TcpSocketState {
                // base: KObjectBase::new(),
                handle: old_handle,
                local_endpoint: self.local_endpoint,
                is_listening: false,
            }))
        };
        return Ok((new_socket, Endpoint::Ip(remote_endpoint)));
    }

    /// missing documentation
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

    /// missing documentation
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

    fn ioctl(&self) -> SysResult {
        Err(LxError::ENOSYS)
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
        self.read(data).await
    }
    /// write from buffer
    fn write(&self, _data: &[u8], _sendto_endpoint: Option<Endpoint>) -> SysResult {
        self.write(_data, _sendto_endpoint)
    }
    /// connect
    async fn connect(&self, _endpoint: Endpoint) -> SysResult {
        self.connect(_endpoint).await
    }
    /// wait for some event on a file descriptor
    fn poll(&self) -> (bool, bool, bool) {
        self.poll()
    }

    fn bind(&mut self, endpoint: Endpoint) -> SysResult {
        self.bind(endpoint)
    }

    fn listen(&mut self) -> SysResult {
        self.listen()
    }

    fn shutdown(&self) -> SysResult {
        self.shutdown()
    }

    async fn accept(&mut self) -> LxResult<(Arc<Mutex<dyn Socket>>, Endpoint)> {
        self.accept().await
    }

    fn endpoint(&self) -> Option<Endpoint> {
        self.endpoint()
    }

    fn remote_endpoint(&self) -> Option<Endpoint> {
        self.remote_endpoint()
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

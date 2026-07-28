// udpsocket

use crate::error::{LxError, LxResult};
use crate::fs::{FileLike, OpenFlags, PollStatus};
use crate::net::*;
use alloc::{boxed::Box, sync::Arc, vec};
use async_trait::async_trait;
// use core::{mem::size_of, slice};
// use kernel_hal::net::get_net_device;
use lock::Mutex;
use smoltcp::socket::{UdpPacketMetadata, UdpSocket, UdpSocketBuffer};
use smoltcp::wire::{IpAddress, Ipv4Address, Ipv6Address};
// use smoltcp::wire::{IpCidr, Ipv4Address, Ipv4Cidr};

// third part
#[allow(unused_imports)]
use zircon_object::impl_kobject;
#[allow(unused_imports)]
use zircon_object::object::*;

pub struct UdpSocketState {
    /// Kernel object base
    base: KObjectBase,
    /// UdpSocket Inner
    inner: Arc<Mutex<UdpInner>>,
}

/// UDP socket inner
#[derive(Debug)]
pub struct UdpInner {
    /// A wrapper for `SocketHandle`
    handle: GlobalSocketHandle,
    /// remember remote endpoint for connect fn
    remote_endpoint: Option<IpEndpoint>,
    /// flags on the socket
    flags: OpenFlags,
    /// ipv6 domain socket flag
    ipv6: bool,
}

// Moved to mod.rs as public constants

// Moved to mod.rs as public structures

// Helpers moved to mod.rs

impl UdpSocketState {
    /// missing documentation
    pub fn new(ipv6: bool) -> LxResult<Self> {
        info!("udp new");
        let rx_buffer = UdpSocketBuffer::new(
            vec![UdpPacketMetadata::EMPTY; UDP_METADATA_BUF],
            vec![0; UDP_RECVBUF],
        );
        let tx_buffer = UdpSocketBuffer::new(
            vec![UdpPacketMetadata::EMPTY; UDP_METADATA_BUF],
            vec![0; UDP_SENDBUF],
        );
        let socket = UdpSocket::new(rx_buffer, tx_buffer);
        let handle = super::register_smoltcp_socket(socket)?;

        Ok(UdpSocketState {
            base: KObjectBase::new(),
            inner: Arc::new(Mutex::new(UdpInner {
                handle,
                remote_endpoint: None,
                flags: OpenFlags::RDWR,
                ipv6,
            })),
        })
    }

    fn family_addr(ipv6: bool) -> IpAddress {
        if ipv6 {
            IpAddress::Ipv6(Ipv6Address::UNSPECIFIED)
        } else {
            IpAddress::Ipv4(Ipv4Address::UNSPECIFIED)
        }
    }

    fn endpoint_matches_family(ipv6: bool, ep: &IpEndpoint) -> bool {
        matches!(
            (ipv6, ep.addr),
            (true, IpAddress::Ipv6(_)) | (false, IpAddress::Ipv4(_))
        )
    }
}

/// missing in implementation
#[async_trait]
impl Socket for UdpSocketState {
    /// read to buffer
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        info!("udp read");
        let (handle, non_block) = {
            let inner = self.inner.lock();
            (inner.handle.0, inner.flags.contains(OpenFlags::NON_BLOCK))
        };
        loop {
            let sets = get_sockets();
            let mut sets = sets.lock();
            let mut socket = sets.get::<UdpSocket>(handle);
            let copied_len = socket.recv_slice(data);
            drop(socket);
            drop(sets);

            match copied_len {
                Ok((size, endpoint)) => return (Ok(size), Endpoint::Ip(endpoint)),
                Err(smoltcp::Error::Exhausted) => {
                    drain_net_poll(4);
                    // The receive buffer is empty. Try again later...
                    if non_block {
                        debug!("NON_BLOCK: Try again later...");
                        return (Err(LxError::EAGAIN), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                    } else {
                        trace!("udp Exhausted. try again")
                    }
                }
                Err(err) => {
                    error!("udp socket recv_slice error: {:?}", err);
                    return (
                        Err(LxError::ENOTCONN),
                        Endpoint::Ip(IpEndpoint::UNSPECIFIED),
                    );
                }
            }
            if let Err(e) = crate::process::check_and_deliver_tty_interrupt() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            // Also honor non-TTY signals (SIGTERM/SIGALRM/...) so a blocked
            // recvfrom returns EINTR, matching the icmp socket path.
            if let Err(e) = crate::process::check_signals() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            kernel_hal::deferred_job::drain_deferred_jobs();
            // Park on the RX IRQ waker (5 ms fallback) instead of busy-spinning
            // — an idle udhcpc blocked on recvfrom otherwise pegs a core.
            kernel_hal::net::NetRxOrTimeoutFuture::new(5).await;
        }
    }
    async fn peek(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        let (handle, non_block) = {
            let inner = self.inner.lock();
            (inner.handle.0, inner.flags.contains(OpenFlags::NON_BLOCK))
        };
        loop {
            let sets = get_sockets();
            let mut sets = sets.lock();
            let mut socket = sets.get::<UdpSocket>(handle);
            // peek_slice returns &IpEndpoint which borrows `socket`.
            // Dereference (copy) it immediately so the borrow ends inside
            // this block, allowing drop(socket) below.
            let copied_len: Result<(usize, IpEndpoint), _> =
                socket.peek_slice(data).map(|(n, ep)| (n, *ep));
            drop(socket);
            drop(sets);

            match copied_len {
                Ok((size, endpoint)) => return (Ok(size), Endpoint::Ip(endpoint)),
                Err(smoltcp::Error::Exhausted) => {
                    drain_net_poll(4);
                    if non_block {
                        return (Err(LxError::EAGAIN), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                    }
                }
                Err(err) => {
                    error!("udp socket peek_slice error: {:?}", err);
                    return (
                        Err(LxError::ENOTCONN),
                        Endpoint::Ip(IpEndpoint::UNSPECIFIED),
                    );
                }
            }
            if let Err(e) = crate::process::check_and_deliver_tty_interrupt() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            // Also honor non-TTY signals (SIGTERM/SIGALRM/...) so a blocked
            // recvfrom returns EINTR, matching the icmp socket path.
            if let Err(e) = crate::process::check_signals() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            kernel_hal::deferred_job::drain_deferred_jobs();
            // Park on the RX IRQ waker (5 ms fallback) instead of busy-spinning
            // — an idle udhcpc blocked on recvfrom otherwise pegs a core.
            kernel_hal::net::NetRxOrTimeoutFuture::new(5).await;
        }
    }
    /// write from buffer
    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult {
        info!("udp write");
        let inner = self.inner.lock();
        let remote_endpoint = {
            if let Some(Endpoint::Ip(ref endpoint)) = sendto_endpoint {
                endpoint
            } else if let Some(ref endpoint) = inner.remote_endpoint {
                endpoint
            } else {
                return Err(LxError::ENOTCONN);
            }
        };
        if !Self::endpoint_matches_family(inner.ipv6, remote_endpoint) {
            return Err(LxError::EINVAL);
        }

        let sets = get_sockets();
        let mut sets = sets.lock();
        let mut socket = sets.get::<UdpSocket>(inner.handle.0);
        if socket.endpoint().port == 0 {
            if let Err(e) = socket.bind(IpEndpoint::new(
                Self::family_addr(inner.ipv6),
                get_ephemeral_port(),
            )) {
                warn!("udp bind failed: {:?}", e);
                drop(socket);
                drop(sets);
                return Err(LxError::EINVAL);
            }
        }

        let _len = socket.send_slice(data, *remote_endpoint);

        drop(socket);
        drop(sets);
        flush_socket_egress();

        match _len {
            Ok(()) => Ok(data.len()),
            Err(err) => {
                warn!("udp send_slice failed: {:?}", err);
                Err(LxError::EIO)
            }
        }
    }
    /// connect
    async fn connect(&self, endpoint: Endpoint) -> SysResult {
        if let Endpoint::Ip(ip) = endpoint {
            let is_ipv6 = self.inner.lock().ipv6;
            if !Self::endpoint_matches_family(is_ipv6, &ip) {
                return Err(LxError::EINVAL);
            }
            let handle = self.inner.lock().handle.0;
            let sockets = get_sockets();
            let mut set = sockets.lock();
            let mut socket = set.get::<UdpSocket>(handle);
            if socket.endpoint().port == 0 {
                if let Err(e) = socket.bind(IpEndpoint::new(
                    Self::family_addr(is_ipv6),
                    get_ephemeral_port(),
                )) {
                    warn!("udp connect: implicit bind failed: {:?}", e);
                    return Err(LxError::EINVAL);
                }
            }
            drop(socket);
            drop(set);

            self.inner.lock().remote_endpoint = Some(ip);
            Ok(0)
        } else {
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
            let socket = sets.get::<UdpSocket>(inner.handle.0);
            (socket.can_recv(), socket.can_send())
        };
        if (events.contains(PollEvents::IN) && !recv_state)
            || (events.contains(PollEvents::OUT) && !send_state)
        {
            crate::net::drain_net_tick();
        }

        let (mut input, mut output, mut err) = (false, false, false);
        let sets = get_sockets();
        let mut sets = sets.lock();
        let socket = sets.get::<UdpSocket>(inner.handle.0);
        if !socket.is_open() {
            err = true;
        } else {
            if socket.can_recv() {
                input = true;
            }
            if socket.can_send() {
                output = true;
            }
        }
        debug!("udp poll: {:?}", (input, output, err));
        (input, output, err)
    }

    fn bind(&self, endpoint: Endpoint) -> SysResult {
        info!("udp bind");
        #[allow(irrefutable_let_patterns)]
        if let Endpoint::Ip(mut ip) = endpoint {
            let is_ipv6 = self.inner.lock().ipv6;
            if !Self::endpoint_matches_family(is_ipv6, &ip) {
                return Err(LxError::EINVAL);
            }
            if ip.port == 0 {
                ip.port = get_ephemeral_port();
            }
            let sockets = get_sockets();
            let mut set = sockets.lock();
            let mut socket = set.get::<UdpSocket>(self.inner.lock().handle.0);
            match socket.bind(ip) {
                Ok(()) => {
                    drop(socket);
                    drop(set);
                    crate::net::drain_net_urgent();
                    Ok(0)
                }
                Err(_) => Err(LxError::EINVAL),
            }
        } else {
            Err(LxError::EINVAL)
        }
    }
    fn listen(&self) -> SysResult {
        warn!("listen is unimplemented");
        Err(LxError::EINVAL)
    }
    fn shutdown(&self, _howto: usize) -> SysResult {
        warn!("shutdown is unimplemented");
        Err(LxError::EINVAL)
    }
    async fn accept(&self) -> LxResult<(Arc<dyn FileLike>, Endpoint)> {
        warn!("accept is unimplemented");
        Err(LxError::EINVAL)
    }
    fn endpoint(&self) -> Option<Endpoint> {
        // Copy handle/ipv6 out of `inner` and drop the guard before locking the
        // global socket set. The hot paths (poll/write) take inner->SOCKETS;
        // locking SOCKETS->inner here is an ABBA inversion that deadlocks two
        // threads sharing one fd (spinlocks).
        let (handle, ipv6) = {
            let inner = self.inner.lock();
            (inner.handle.0, inner.ipv6)
        };
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let socket = sockets.get::<UdpSocket>(handle);
        let ep = socket.endpoint();
        let addr = if ep.addr.is_unspecified() {
            if ipv6 {
                IpAddress::Ipv6(Ipv6Address::UNSPECIFIED)
            } else {
                IpAddress::Ipv4(Ipv4Address::UNSPECIFIED)
            }
        } else {
            ep.addr
        };
        Some(Endpoint::Ip(IpEndpoint::new(addr, ep.port)))
    }
    fn remote_endpoint(&self) -> Option<Endpoint> {
        let inner = self.inner.lock();
        inner.remote_endpoint.map(|ep| {
            let addr = if ep.addr.is_unspecified() {
                if inner.ipv6 {
                    IpAddress::Ipv6(Ipv6Address::UNSPECIFIED)
                } else {
                    IpAddress::Ipv4(Ipv4Address::UNSPECIFIED)
                }
            } else {
                ep.addr
            };
            Endpoint::Ip(IpEndpoint::new(addr, ep.port))
        })
    }
    fn setsockopt(&self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        warn!("setsockopt is unimplemented");
        Ok(0)
    }

    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> SysResult {
        // trace, not warn: this fires on every SIOCGIFFLAGS/ifconfig poll and
        // floods the console during a download, burying the [tcp read] STALL
        // diagnostic that actually matters.
        trace!("UdpSocket: ioctl request={:#x}, arg1={:#x}", request, arg1);
        let ipv6 = self.inner.lock().ipv6;
        handle_net_ioctl(request, arg1, arg2, arg3, ipv6)
    }

    fn get_buffer_capacity(&self) -> Option<(usize, usize)> {
        // Read the handle and drop the `inner` guard before locking SOCKETS to
        // preserve the inner->SOCKETS order (avoids the ABBA deadlock).
        let handle = self.inner.lock().handle.0;
        let sockets = get_sockets();
        let mut set = sockets.lock();
        let socket = set.get::<UdpSocket>(handle);
        let (recv_ca, send_ca) = (
            socket.payload_recv_capacity(),
            socket.payload_send_capacity(),
        );
        Some((recv_ca, send_ca))
    }

    fn socket_type(&self) -> Option<SocketType> {
        Some(SocketType::SOCK_DGRAM)
    }
}

impl_kobject!(UdpSocketState);

#[async_trait]
impl FileLike for UdpSocketState {
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

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        Socket::read(self, buf).await.0
    }

    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
        // Sockets do not support positioned reads.
        Err(LxError::ESPIPE)
    }

    fn write(&self, buf: &[u8]) -> LxResult<usize> {
        Socket::write(self, buf, None)
    }

    fn poll(&self, events: PollEvents) -> LxResult<PollStatus> {
        let (read, write, error) = Socket::poll(self, events);
        Ok(PollStatus { read, write, error })
    }

    async fn async_poll(&self, events: PollEvents) -> LxResult<PollStatus> {
        let (mut read, mut write, mut error) = Socket::poll(self, events);
        let ready = (events.contains(PollEvents::IN) && read)
            || (events.contains(PollEvents::OUT) && write)
            || error;
        if !ready {
            kernel_hal::net::NetRxOrTimeoutFuture::new(5).await;
            (read, write, error) = Socket::poll(self, events);
        }
        Ok(PollStatus { read, write, error })
    }

    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize> {
        Socket::ioctl(self, request, arg1, arg2, arg3)
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            inner: self.inner.clone(),
        })
    }

    fn as_socket(&self) -> LxResult<&dyn Socket> {
        Ok(self)
    }
}

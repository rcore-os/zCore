use crate::{
    error::{LxError, LxResult},
    fs::{FileLike, OpenFlags, PollEvents, PollStatus},
    net::*,
};
use alloc::sync::Arc;
use async_trait::async_trait;
use lock::Mutex;
use smoltcp::{
    socket::{RawPacketMetadata, RawSocket, RawSocketBuffer},
    wire::{IpProtocol, IpVersion, Ipv4Address, Ipv4Packet, Ipv6Address, Ipv6Packet, Ipv6Repr},
};

#[allow(unused_imports)]
use zircon_object::object::*;

pub struct RawSocketState {
    base: KObjectBase,
    inner: Arc<RawSocketInner>,
}

#[derive(Debug)]
struct RawSocketInner {
    handle: GlobalSocketHandle,
    header_included: Mutex<bool>,
    flags: Mutex<OpenFlags>,
    remote: Mutex<Option<Endpoint>>,
    ipv6: bool,
}

impl RawSocketState {
    /// missing documentation
    pub fn new(protocol: u8, ipv6: bool) -> LxResult<Self> {
        let rx_buffer = RawSocketBuffer::new(
            vec![RawPacketMetadata::EMPTY; RAW_METADATA_BUF],
            vec![0; RAW_RECVBUF],
        );
        let tx_buffer = RawSocketBuffer::new(
            vec![RawPacketMetadata::EMPTY; RAW_METADATA_BUF],
            vec![0; RAW_SENDBUF],
        );
        let socket = RawSocket::new(
            if ipv6 {
                IpVersion::Ipv6
            } else {
                IpVersion::Ipv4
            },
            IpProtocol::from(protocol),
            rx_buffer,
            tx_buffer,
        );
        let handle = super::register_smoltcp_socket(socket)?;

        Ok(RawSocketState {
            base: KObjectBase::new(),
            inner: Arc::new(RawSocketInner {
                handle,
                header_included: Mutex::new(false),
                flags: Mutex::new(OpenFlags::RDWR),
                remote: Mutex::new(None),
                ipv6,
            }),
        })
    }
}

/// missing in implementation
#[async_trait]
impl Socket for RawSocketState {
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        loop {
            drain_net_poll(4);
            if let Err(e) = crate::process::check_and_deliver_tty_interrupt() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            // Also honor non-TTY signals (SIGTERM/SIGALRM/...) so a blocked raw
            // recv returns EINTR, matching the icmp socket path.
            if let Err(e) = crate::process::check_signals() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            let remote = self.inner.remote.lock().as_ref().and_then(|ep| match ep {
                Endpoint::Ip(ip) => Some(ip.addr),
                _ => None,
            });
            if !self.inner.ipv6 {
                let proto = {
                    let net_sockets = get_sockets();
                    let mut sockets = net_sockets.lock();
                    let proto = sockets.get::<RawSocket>(self.inner.handle.0).ip_protocol();
                    proto
                };
                if proto == IpProtocol::Icmp {
                    if let Some((n, src)) = super::icmp_rx::pop_ipv4_raw_reply(remote, data) {
                        return (Ok(n), Endpoint::Ip(IpEndpoint::new(src, 0)));
                    }
                    // RxToken also enqueues the same frame on smoltcp RawSocket; drop it
                    // so BusyBox ping does not report (DUP!) with a second TTL.
                    {
                        let net_sockets = get_sockets();
                        let mut sockets = net_sockets.lock();
                        let mut socket = sockets.get::<RawSocket>(self.inner.handle.0);
                        if socket.can_recv() {
                            let _ = socket.recv_slice(data);
                        }
                    }
                    let non_block = self.inner.flags.lock().contains(OpenFlags::NON_BLOCK);
                    if non_block {
                        return (Err(LxError::EAGAIN), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                    }
                    kernel_hal::thread::sleep_until(
                        kernel_hal::timer::timer_now() + core::time::Duration::from_millis(10),
                    )
                    .await;
                    continue;
                }
            }
            let net_sockets = get_sockets();
            let mut sockets = net_sockets.lock();
            let mut socket = sockets.get::<RawSocket>(self.inner.handle.0);
            if socket.can_recv() {
                if let Ok(size) = socket.recv_slice(data) {
                    drop(socket);
                    drop(sockets);
                    if self.inner.ipv6 {
                        if let Ok(packet) = Ipv6Packet::new_checked(&data[..size]) {
                            let src_addr = packet.src_addr();
                            let payload_len = size.saturating_sub(40);
                            data.copy_within(40..size, 0);
                            return (
                                Ok(payload_len),
                                Endpoint::Ip(IpEndpoint {
                                    addr: IpAddress::Ipv6(src_addr),
                                    port: 0,
                                }),
                            );
                        }
                    } else {
                        if let Ok(packet) = Ipv4Packet::new_checked(&data[..size]) {
                            return (
                                Ok(size),
                                Endpoint::Ip(IpEndpoint {
                                    addr: IpAddress::Ipv4(packet.src_addr()),
                                    port: 0,
                                }),
                            );
                        }
                    }
                    return (Err(LxError::EINVAL), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                }
            }
            let non_block = self.inner.flags.lock().contains(OpenFlags::NON_BLOCK);
            drop(socket);
            drop(sockets);
            if non_block {
                return (Err(LxError::EAGAIN), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            kernel_hal::thread::sleep_until(
                kernel_hal::timer::timer_now() + core::time::Duration::from_millis(10),
            )
            .await;
        }
    }

    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult {
        let endpoint = match sendto_endpoint {
            Some(ep) => Some(ep),
            None => self.inner.remote.lock().clone(),
        };
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let mut socket = sockets.get::<RawSocket>(self.inner.handle.0);
        if *self.inner.header_included.lock() {
            let result = match socket.send_slice(data) {
                Ok(()) => Ok(data.len()),
                Err(_) => Err(LxError::ENOBUFS),
            };
            drop(socket);
            drop(sockets);
            if result.is_ok() {
                flush_socket_egress();
            }
            return result;
        }
        let Endpoint::Ip(ip) = endpoint.ok_or(LxError::ENOTCONN)? else {
            return Err(LxError::EINVAL);
        };
        if !*self.inner.header_included.lock()
            && !self.inner.ipv6
            && socket.ip_protocol() == IpProtocol::Icmp
        {
            let IpAddress::Ipv4(dst) = ip.addr else {
                return Err(LxError::EINVAL);
            };
            if dst.is_loopback() || is_local_host_ipv4(dst) {
                super::icmp_rx::queue_echo_reply(IpAddress::Ipv4(dst), data.to_vec());
                return Ok(data.len());
            }
        }
        if self.inner.ipv6 {
            let IpAddress::Ipv6(dst) = ip.addr else {
                return Err(LxError::EINVAL);
            };
            let src = select_ipv6_for_dst(dst);
            if src.is_unspecified() {
                return Err(LxError::EINVAL);
            }

            // The IPv6 payload length is a 16-bit field; a larger payload would
            // wrap on `set_payload_len(len as u16)` and make `payload_mut()`
            // shorter than `data`, panicking the `copy_from_slice` below.
            if data.len() > u16::MAX as usize {
                return Err(LxError::EMSGSIZE);
            }
            let len = data.len();
            let mut buffer = vec![0u8; len + 40];
            let mut packet = Ipv6Packet::new_unchecked(&mut buffer);
            let ip_repr = Ipv6Repr {
                src_addr: src,
                dst_addr: dst,
                next_header: socket.ip_protocol(),
                payload_len: len,
                hop_limit: 64,
            };
            ip_repr.emit(&mut packet);
            packet.payload_mut().copy_from_slice(data);

            if socket.ip_protocol() == IpProtocol::Icmpv6 {
                let mut icmp_pkt = smoltcp::wire::Icmpv6Packet::new_unchecked(packet.payload_mut());
                icmp_pkt.fill_checksum(&IpAddress::Ipv6(src), &IpAddress::Ipv6(dst));
            }

            socket.send_slice(&buffer).map_err(|e| {
                warn!("raw socket send_slice failed: {:?}", e);
                LxError::ENOBUFS
            })?;

            drop(socket);
            drop(sockets);
            flush_socket_egress();
            Ok(len)
        } else {
            let IpAddress::Ipv4(mut v4_dst) = ip.addr else {
                return Err(LxError::EINVAL);
            };
            if v4_dst.is_unspecified() {
                v4_dst = Ipv4Address::new(127, 0, 0, 1);
            }
            if !v4_dst.is_unicast() && !v4_dst.is_broadcast() && !v4_dst.is_multicast() {
                warn!("raw socket: invalid destination address {:?}", v4_dst);
                return Err(LxError::EINVAL);
            }

            // The IPv4 total-length field is 16 bits (header + payload); a larger
            // payload would wrap on `set_total_len((20 + len) as u16)` and make
            // `payload_mut()` shorter than `data`, panicking `copy_from_slice`.
            if data.len() > (u16::MAX as usize - 20) {
                return Err(LxError::EMSGSIZE);
            }
            let len = data.len();
            let mut buffer = vec![0u8; len + 20];
            let mut packet = Ipv4Packet::new_unchecked(&mut buffer);
            packet.set_version(4);
            packet.set_header_len(20);
            packet.set_total_len((20 + len) as u16);
            packet.set_protocol(socket.ip_protocol());
            let src_addr = select_ipv4_for_dst(v4_dst);
            if src_addr.is_unspecified() {
                return Err(LxError::EINVAL);
            }
            packet.set_src_addr(src_addr);
            packet.set_dst_addr(v4_dst);
            packet.set_hop_limit(64);
            packet.payload_mut().copy_from_slice(data);
            packet.fill_checksum();

            socket.send_slice(&buffer).map_err(|e| {
                warn!("raw socket send_slice failed: {:?}", e);
                LxError::ENOBUFS
            })?;

            drop(socket);
            drop(sockets);
            flush_socket_egress();
            Ok(len)
        }
    }

    async fn connect(&self, endpoint: Endpoint) -> SysResult {
        let Endpoint::Ip(ip) = endpoint else {
            return Err(LxError::EINVAL);
        };
        let family_ok = matches!(
            (self.inner.ipv6, ip.addr),
            (true, IpAddress::Ipv6(_)) | (false, IpAddress::Ipv4(_))
        );
        if !family_ok {
            return Err(LxError::EINVAL);
        }
        *self.inner.remote.lock() = Some(Endpoint::Ip(ip));
        Ok(0)
    }

    fn bind(&self, _endpoint: Endpoint) -> SysResult {
        Ok(0)
    }

    fn setsockopt(&self, level: usize, opt: usize, data: &[u8]) -> SysResult {
        if let (IPPROTO_IP, IP_HDRINCL) = (level, opt) {
            if let Some(arg) = data.first() {
                *self.inner.header_included.lock() = *arg > 0;
                debug!("hdrincl set to {}", *self.inner.header_included.lock());
            }
        }
        Ok(0)
    }
    fn get_buffer_capacity(&self) -> Option<(usize, usize)> {
        let sockets = get_sockets();
        let mut s = sockets.lock();
        let socket = s.get::<RawSocket>(self.inner.handle.0);
        let (recv_ca, send_ca) = (
            socket.payload_recv_capacity(),
            socket.payload_send_capacity(),
        );
        Some((recv_ca, send_ca))
    }
    fn endpoint(&self) -> Option<Endpoint> {
        let addr = if self.inner.ipv6 {
            IpAddress::Ipv6(Ipv6Address::UNSPECIFIED)
        } else {
            IpAddress::Ipv4(Ipv4Address::UNSPECIFIED)
        };
        Some(Endpoint::Ip(IpEndpoint { addr, port: 0 }))
    }
    fn remote_endpoint(&self) -> Option<Endpoint> {
        self.inner.remote.lock().clone()
    }
    fn socket_type(&self) -> Option<SocketType> {
        Some(SocketType::SOCK_RAW)
    }

    fn poll(&self, _events: PollEvents) -> (bool, bool, bool) {
        drain_net_poll(1);
        let s = get_sockets();
        let mut s = s.lock();
        let socket = s.get::<RawSocket>(self.inner.handle.0);
        let readable = socket.can_recv()
            || (!self.inner.ipv6
                && socket.ip_protocol() == IpProtocol::Icmp
                && super::icmp_rx::pending_for(false));
        (readable, socket.can_send(), false)
    }
}

zircon_object::impl_kobject!(RawSocketState);

#[async_trait]
impl FileLike for RawSocketState {
    fn flags(&self) -> OpenFlags {
        *self.inner.flags.lock()
    }

    fn set_flags(&self, f: OpenFlags) -> LxResult {
        let mut flags = self.inner.flags.lock();
        flags.set(OpenFlags::APPEND, f.contains(OpenFlags::APPEND));
        flags.set(OpenFlags::NON_BLOCK, f.contains(OpenFlags::NON_BLOCK));
        flags.set(OpenFlags::CLOEXEC, f.contains(OpenFlags::CLOEXEC));
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            inner: self.inner.clone(),
        })
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
        let (read, write, error) = Socket::poll(self, events);
        Ok(PollStatus { read, write, error })
    }

    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize> {
        handle_net_ioctl(request, arg1, arg2, arg3, self.inner.ipv6)
    }

    fn as_socket(&self) -> LxResult<&dyn Socket> {
        Ok(self)
    }
}

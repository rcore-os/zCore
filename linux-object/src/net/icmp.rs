//! ICMP sockets (Linux `SOCK_DGRAM` + `IPPROTO_ICMP`) via smoltcp — same stack as TCP/UDP.

use crate::{
    error::{LxError, LxResult},
    fs::{FileLike, OpenFlags, PollEvents, PollStatus},
    net::*,
};
use alloc::sync::Arc;
use alloc::vec;
use async_trait::async_trait;
use lock::Mutex;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::socket::{IcmpEndpoint, IcmpPacketMetadata, IcmpSocket, IcmpSocketBuffer};
use smoltcp::wire::{
    Icmpv4Packet, Icmpv4Repr, Icmpv6Packet, Icmpv6Repr, IpAddress, IpEndpoint, Ipv6Address,
};

#[allow(unused_imports)]
use zircon_object::object::*;

const ICMP_PACKET_META: usize = 64;

fn icmp_checksum_caps() -> ChecksumCapabilities {
    ChecksumCapabilities::default()
}

pub struct IcmpSocketState {
    base: KObjectBase,
    inner: Arc<Mutex<IcmpInner>>,
}

#[derive(Debug)]
struct IcmpInner {
    handle: GlobalSocketHandle,
    remote: Option<IpEndpoint>,
    flags: OpenFlags,
    echo_seq: u16,
    echo_id: u16,
    ipv6: bool,
}

impl IcmpSocketState {
    pub fn new(ipv6: bool) -> LxResult<Self> {
        let rx_buffer = IcmpSocketBuffer::new(
            vec![IcmpPacketMetadata::EMPTY; ICMP_PACKET_META],
            vec![0u8; ICMP_RECVBUF.min(64 * 1024)],
        );
        let tx_buffer = IcmpSocketBuffer::new(
            vec![IcmpPacketMetadata::EMPTY; ICMP_PACKET_META],
            vec![0u8; ICMP_SENDBUF.min(64 * 1024)],
        );
        let socket = IcmpSocket::new(rx_buffer, tx_buffer);
        let handle = register_smoltcp_socket(socket)?;
        Ok(Self {
            base: KObjectBase::new(),
            inner: Arc::new(Mutex::new(IcmpInner {
                handle,
                remote: None,
                flags: OpenFlags::RDWR,
                echo_seq: 0,
                echo_id: (kernel_hal::timer::timer_now().as_micros() as u16).wrapping_add(1),
                ipv6,
            })),
        })
    }

    fn bind_ident(inner: &IcmpInner) -> LxResult<()> {
        let sets = get_sockets();
        let mut sets = sets.lock();
        let mut sock = sets.get::<IcmpSocket>(inner.handle.0);
        if sock.is_open() {
            return Ok(());
        }
        sock.bind(IcmpEndpoint::Ident(inner.echo_id))
            .map_err(|_| LxError::EINVAL)
    }

    /// Build ICMP echo request bytes (type 8/128 + id + seq + payload).
    fn echo_request_bytes(inner: &mut IcmpInner, data: &[u8]) -> alloc::vec::Vec<u8> {
        let expect_type = if inner.ipv6 { 128 } else { 8 };
        if data.len() >= 8 && data[0] == expect_type && data[1] == 0 {
            return data.to_vec();
        }
        let mut out = vec![0u8; 8 + data.len()];
        out[0] = expect_type;
        out[1] = 0;
        out[4..6].copy_from_slice(&inner.echo_id.to_be_bytes());
        out[6..8].copy_from_slice(&inner.echo_seq.to_be_bytes());
        inner.echo_seq = inner.echo_seq.wrapping_add(1);
        out[8..].copy_from_slice(data);
        out
    }

    fn send_echo(inner: &mut IcmpInner, dst: IpAddress, icmp_bytes: &[u8]) -> LxResult {
        Self::bind_ident(inner)?;
        let caps = icmp_checksum_caps();
        let sets = get_sockets();
        let mut sets = sets.lock();
        let mut sock = sets.get::<IcmpSocket>(inner.handle.0);

        match (inner.ipv6, dst) {
            (false, IpAddress::Ipv4(dst_v4)) => {
                if icmp_bytes.len() < 8 {
                    return Err(LxError::EINVAL);
                }
                let ident = u16::from_be_bytes([icmp_bytes[4], icmp_bytes[5]]);
                let seq_no = u16::from_be_bytes([icmp_bytes[6], icmp_bytes[7]]);
                let echo_data = &icmp_bytes[8..];
                let repr = Icmpv4Repr::EchoRequest {
                    ident,
                    seq_no,
                    data: echo_data,
                };
                let buf = sock
                    .send(repr.buffer_len(), IpAddress::Ipv4(dst_v4))
                    .map_err(|_| LxError::ENOBUFS)?;
                let mut pkt = Icmpv4Packet::new_unchecked(buf);
                repr.emit(&mut pkt, &caps);
            }
            (true, IpAddress::Ipv6(dst_v6)) => {
                if icmp_bytes.len() < 8 {
                    return Err(LxError::EINVAL);
                }
                let src = select_ipv6_for_dst(dst_v6);
                if src.is_unspecified() {
                    return Err(LxError::EINVAL);
                }
                let ident = u16::from_be_bytes([icmp_bytes[4], icmp_bytes[5]]);
                let seq_no = u16::from_be_bytes([icmp_bytes[6], icmp_bytes[7]]);
                let echo_data = &icmp_bytes[8..];
                let repr = Icmpv6Repr::EchoRequest {
                    ident,
                    seq_no,
                    data: echo_data,
                };
                let buf = sock
                    .send(repr.buffer_len(), IpAddress::Ipv6(dst_v6))
                    .map_err(|_| LxError::ENOBUFS)?;
                let mut pkt = Icmpv6Packet::new_unchecked(buf);
                repr.emit(
                    &IpAddress::Ipv6(src),
                    &IpAddress::Ipv6(dst_v6),
                    &mut pkt,
                    &caps,
                );
            }
            _ => return Err(LxError::EINVAL),
        }
        drop(sock);
        drop(sets);
        prepare_ipv4_stack();
        flush_socket_egress();
        Ok(())
    }
}

#[async_trait]
impl Socket for IcmpSocketState {
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        loop {
            let (ipv6, remote, non_block) = {
                let inner = self.inner.lock();
                (
                    inner.ipv6,
                    inner.remote.map(|e| e.addr),
                    inner.flags.contains(OpenFlags::NON_BLOCK),
                )
            };

            drain_all_nic_rx();
            if let Some((pkt, src)) = icmp_rx::pop_for(ipv6, remote) {
                let n = pkt.len().min(data.len());
                data[..n].copy_from_slice(&pkt[..n]);
                return (Ok(n), Endpoint::Ip(IpEndpoint::new(src, 0)));
            }

            let handle = self.inner.lock().handle.0;
            let copied = {
                let sets = get_sockets();
                let mut sets = sets.lock();
                let mut sock = sets.get::<IcmpSocket>(handle);
                sock.recv_slice(data)
            };
            match copied {
                Ok((n, src)) => {
                    return (Ok(n), Endpoint::Ip(IpEndpoint::new(src, 0)));
                }
                Err(smoltcp::Error::Exhausted) => {
                    drain_net_urgent();
                    drain_all_nic_rx();
                    if let Some((pkt, src)) = icmp_rx::pop_for(ipv6, remote) {
                        let n = pkt.len().min(data.len());
                        data[..n].copy_from_slice(&pkt[..n]);
                        return (Ok(n), Endpoint::Ip(IpEndpoint::new(src, 0)));
                    }
                    if non_block {
                        return (Err(LxError::EAGAIN), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                    }
                }
                Err(_) => {
                    return (Err(LxError::EIO), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                }
            }

            if let Err(e) = crate::process::check_and_deliver_tty_interrupt() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            if let Err(e) = crate::process::check_signals() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }

            kernel_hal::net::NetRxOrTimeoutFuture::new(25).await;
        }
    }

    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult {
        let endpoint = match sendto_endpoint {
            Some(ep) => Some(ep),
            None => {
                let inner = self.inner.lock();
                inner.remote.map(Endpoint::Ip)
            }
        };
        let Endpoint::Ip(ip) = endpoint.ok_or(LxError::ENOTCONN)? else {
            return Err(LxError::EINVAL);
        };

        let mut inner = self.inner.lock();
        if inner.ipv6 {
            let IpAddress::Ipv6(dst) = ip.addr else {
                return Err(LxError::EINVAL);
            };
            if !dst.is_unicast() {
                return Err(LxError::EINVAL);
            }
            if is_local_host_ipv6(dst) {
                let icmp = Self::echo_request_bytes(&mut inner, data);
                icmp_rx::queue_echo_reply(IpAddress::Ipv6(dst), icmp);
                return Ok(data.len());
            }
            let icmp = Self::echo_request_bytes(&mut inner, data);
            Self::send_echo(&mut inner, IpAddress::Ipv6(dst), &icmp)?;
            Ok(data.len())
        } else {
            let IpAddress::Ipv4(dst) = ip.addr else {
                return Err(LxError::EINVAL);
            };
            if !dst.is_unicast() || is_ipv4_placeholder(dst) || dst.0[0] >= 240 {
                return Err(LxError::EINVAL);
            }
            if dst.is_loopback() || is_local_host_ipv4(dst) {
                let icmp = Self::echo_request_bytes(&mut inner, data);
                icmp_rx::queue_echo_reply(IpAddress::Ipv4(dst), icmp);
                return Ok(data.len());
            }
            let icmp = Self::echo_request_bytes(&mut inner, data);
            Self::send_echo(&mut inner, IpAddress::Ipv4(dst), &icmp)?;
            Ok(data.len())
        }
    }

    async fn connect(&self, endpoint: Endpoint) -> SysResult {
        let Endpoint::Ip(ip) = endpoint else {
            return Err(LxError::EINVAL);
        };
        let family_ok = matches!(
            (self.inner.lock().ipv6, ip.addr),
            (true, IpAddress::Ipv6(_)) | (false, IpAddress::Ipv4(_))
        );
        if !family_ok {
            return Err(LxError::EINVAL);
        }
        let mut inner = self.inner.lock();
        inner.remote = Some(ip);
        Self::bind_ident(&inner)?;
        Ok(0)
    }

    fn bind(&self, _endpoint: Endpoint) -> SysResult {
        let inner = self.inner.lock();
        Self::bind_ident(&inner)?;
        Ok(0)
    }

    fn setsockopt(&self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        Ok(0)
    }

    fn get_buffer_capacity(&self) -> Option<(usize, usize)> {
        Some((ICMP_RECVBUF, ICMP_SENDBUF))
    }

    fn socket_type(&self) -> Option<SocketType> {
        Some(SocketType::SOCK_DGRAM)
    }

    fn poll(&self, _events: PollEvents) -> (bool, bool, bool) {
        kernel_hal::deferred_job::drain_deferred_jobs();
        crate::net::drain_net_tick();
        let inner = self.inner.lock();
        let readable = {
            let sets = get_sockets();
            let mut sets = sets.lock();
            let sock = sets.get::<IcmpSocket>(inner.handle.0);
            sock.can_recv() || icmp_rx::pending_for(inner.ipv6)
        };
        (readable, true, false)
    }
}

fn is_local_host_ipv6(dst: Ipv6Address) -> bool {
    use smoltcp::wire::IpCidr;
    get_net_device().iter().any(|dev| {
        dev.get_ip_address().iter().any(|ip| match ip {
            IpCidr::Ipv6(cidr) => cidr.prefix_len() > 0 && cidr.address() == dst,
            _ => false,
        })
    })
}

zircon_object::impl_kobject!(IcmpSocketState);

#[async_trait]
impl FileLike for IcmpSocketState {
    fn flags(&self) -> OpenFlags {
        self.inner.lock().flags
    }

    fn set_flags(&self, f: OpenFlags) -> LxResult {
        let mut inner = self.inner.lock();
        inner
            .flags
            .set(OpenFlags::APPEND, f.contains(OpenFlags::APPEND));
        inner
            .flags
            .set(OpenFlags::NON_BLOCK, f.contains(OpenFlags::NON_BLOCK));
        inner
            .flags
            .set(OpenFlags::CLOEXEC, f.contains(OpenFlags::CLOEXEC));
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            inner: self.inner.clone(),
        })
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        let inner = self.inner.lock();
        if icmp_rx::pending_for(inner.ipv6) {
            let remote = inner.remote.map(|e| e.addr);
            if let Some((pkt, _src)) = icmp_rx::pop_for(inner.ipv6, remote) {
                let n = pkt.len().min(buf.len());
                buf[..n].copy_from_slice(&pkt[..n]);
                return Ok(n);
            }
        }
        drop(inner);
        Socket::read(self, buf).await.0
    }

    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
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
        kernel_hal::deferred_job::drain_deferred_jobs();
        let (mut read, mut write, mut error) = Socket::poll(self, events);
        let ready = (events.contains(PollEvents::IN) && read)
            || (events.contains(PollEvents::OUT) && write)
            || error;
        if !ready {
            // Park on RX IRQ (fallback timeout) like UDP — avoid busy-spin.
            kernel_hal::net::NetRxOrTimeoutFuture::new(25).await;
            (read, write, error) = Socket::poll(self, events);
        }
        Ok(PollStatus { read, write, error })
    }

    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize> {
        handle_net_ioctl(request, arg1, arg2, arg3, self.inner.lock().ipv6)
    }

    fn as_socket(&self) -> LxResult<&dyn Socket> {
        Ok(self)
    }
}

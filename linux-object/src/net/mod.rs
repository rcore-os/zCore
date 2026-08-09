//! Linux socket objects
//!

/// missing documentation
#[macro_use]
pub mod socket_address;
use crate::error::{LxError, LxResult};
use crate::fs::{FileDesc, FileLike, PollEvents};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use kernel_hal::user::{IoVecOut, UserInOutPtr, UserInPtr};
use log::*;
use smoltcp::wire::{EthernetAddress, IpCidr, IpEndpoint, Ipv4Cidr, Ipv6Cidr};
pub use socket_address::*;

pub fn ifreq_name(raw: &[u8; 16]) -> LxResult<&str> {
    let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    core::str::from_utf8(&raw[..len]).map_err(|_| LxError::EINVAL)
}

fn loopback_tx_handler(packet: &[u8]) {
    let version = packet.first().map(|b| b >> 4).unwrap_or(4);
    info!(
        "[loopback tx] packet version={}, len={}",
        version,
        packet.len()
    );
    let ethertype = if version == 6 { 0x86ddu16 } else { 0x0800u16 };
    // Size the ethernet frame to the actual packet. The old fixed 2 KiB stack
    // buffer truncated any IP packet larger than 2034 bytes, but loopback has a
    // large MTU; a truncated frame whose IP header still advertises the full
    // length is malformed and fails checksum/parse (or loses data) on the RX
    // side. `kernel_vec_zeroed` caps at MAX_KERNEL_VEC and is fallible, so an
    // oversized packet is dropped rather than truncated.
    let mut eth_frame = match kernel_vec_zeroed(14 + packet.len()) {
        Ok(v) => v,
        Err(_) => {
            warn!(
                "[loopback tx] frame alloc failed for {} bytes, dropping",
                packet.len()
            );
            return;
        }
    };
    eth_frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
    eth_frame[14..].copy_from_slice(packet);
    packet::push_packet(&eth_frame);
}

/// Global initialization for the network stack.
pub fn init() {
    zcore_drivers::net::set_packet_callback(packet::push_packet);
    zcore_drivers::net::loopback::register_loopback_tx_callback(loopback_tx_handler);
    refresh_arp_local_macs();
    for iface in get_net_device() {
        ensure_ipv6_link_local(iface.as_ref());
    }
}

pub fn refresh_arp_local_macs() {
    let macs: alloc::vec::Vec<_> = get_net_device().iter().map(|d| d.get_mac()).collect();
    arp_cache::refresh_local_macs(macs.clone());
    ndp_cache::refresh_local_macs(macs);
}

/// Derive the RFC 4862 link-local address (fe80::/64, EUI-64 IID) from a MAC.
pub fn ipv6_link_local_from_mac(mac: &EthernetAddress) -> smoltcp::wire::Ipv6Address {
    let b = mac.as_bytes();
    smoltcp::wire::Ipv6Address::new(
        0xfe80,
        0,
        0,
        0,
        u16::from(b[0] ^ 2) << 8 | u16::from(b[1]),
        u16::from(b[2]) << 8 | 0xff,
        0xfe << 8 | u16::from(b[3]),
        u16::from(b[4]) << 8 | u16::from(b[5]),
    )
}

/// Ensure every Ethernet iface has a link-local IPv6 address (required by DHCPv6).
pub fn ensure_ipv6_link_local(iface: &dyn zcore_drivers::scheme::NetScheme) {
    if iface.get_ifname() == "loopback" {
        return;
    }
    let expected = ipv6_link_local_from_mac(&iface.get_mac());
    let has_ll = iface.get_ip_address().iter().any(|ip| {
        matches!(
            ip,
            IpCidr::Ipv6(cidr) if cidr.address().is_link_local() || cidr.address() == expected
        )
    });
    if !has_ll {
        let ll = IpCidr::Ipv6(Ipv6Cidr::new(expected, 64));
        let _ = iface.add_ip_address(ll);
        info!(
            "[net] ensured link-local {} on {}",
            expected,
            iface.get_ifname()
        );
    }
}

pub fn iface_by_name(ifname: &str) -> LxResult<Arc<dyn zcore_drivers::scheme::NetScheme>> {
    get_net_device()
        .into_iter()
        .find(|iface| iface.get_ifname() == ifname)
        .ok_or(LxError::ENODEV)
}

/// Map Linux `ifindex` (1-based, see `SIOCGIFINDEX`) to our netdev list order.
pub fn iface_by_linux_ifindex(idx: u32) -> LxResult<Arc<dyn zcore_drivers::scheme::NetScheme>> {
    if idx == 0 {
        return Err(LxError::ENODEV);
    }
    get_net_device()
        .get((idx as usize).saturating_sub(1))
        .cloned()
        .ok_or(LxError::ENODEV)
}

/// Pick the Ethernet netdev for an IPv4 destination (never loopback).
pub fn netdev_for_ipv4(
    dst: smoltcp::wire::Ipv4Address,
) -> LxResult<Arc<dyn zcore_drivers::scheme::NetScheme>> {
    use smoltcp::wire::IpCidr;
    let mut best: Option<(u8, Arc<dyn zcore_drivers::scheme::NetScheme>)> = None;
    for iface in get_net_device().iter() {
        if iface.get_ifname() == "loopback" {
            continue;
        }
        for ip in iface.get_ip_address() {
            if let IpCidr::Ipv4(cidr) = ip {
                let addr = cidr.address();
                if cidr.prefix_len() == 0
                    || addr.is_unspecified()
                    || is_ipv4_placeholder(addr)
                    || addr.0[0] >= 240
                {
                    continue;
                }
                if cidr.contains_addr(&dst)
                    && best.as_ref().is_none_or(|(p, _)| cidr.prefix_len() > *p)
                {
                    best = Some((cidr.prefix_len(), iface.clone()));
                }
            }
        }
    }
    if let Some((_, dev)) = best {
        return Ok(dev);
    }
    for iface in get_net_device().iter() {
        if iface.get_ifname() == "loopback" {
            continue;
        }
        let has_v4 = iface.get_ip_address().iter().any(|ip| {
            matches!(
                ip,
                IpCidr::Ipv4(cidr)
                    if cidr.prefix_len() > 0 && !cidr.address().is_unspecified()
            )
        });
        if has_v4 {
            return Ok(iface.clone());
        }
    }
    Err(LxError::ENODEV)
}

/// Pick the Ethernet netdev for an IPv6 destination (never loopback).
pub fn netdev_for_ipv6(
    dst: smoltcp::wire::Ipv6Address,
) -> LxResult<Arc<dyn zcore_drivers::scheme::NetScheme>> {
    use smoltcp::wire::IpCidr;
    let mut best: Option<(u8, Arc<dyn zcore_drivers::scheme::NetScheme>)> = None;
    for iface in get_net_device().iter() {
        if iface.get_ifname() == "loopback" {
            continue;
        }
        for ip in iface.get_ip_address() {
            if let IpCidr::Ipv6(cidr) = ip {
                if cidr.prefix_len() == 0 || cidr.address().is_unspecified() {
                    continue;
                }
                if cidr.contains_addr(&dst)
                    && best.as_ref().is_none_or(|(p, _)| cidr.prefix_len() > *p)
                {
                    best = Some((cidr.prefix_len(), iface.clone()));
                }
            }
        }
    }
    if let Some((_, dev)) = best {
        return Ok(dev);
    }
    for iface in get_net_device().iter() {
        if iface.get_ifname() == "loopback" {
            continue;
        }
        let has_v6 = iface.get_ip_address().iter().any(|ip| {
            matches!(
                ip,
                IpCidr::Ipv6(cidr)
                    if cidr.prefix_len() > 0 && !cidr.address().is_unspecified()
            )
        });
        if has_v6 {
            return Ok(iface.clone());
        }
    }
    Err(LxError::ENODEV)
}

pub fn iface_ipv4_cidr(iface: &dyn zcore_drivers::scheme::NetScheme) -> Option<Ipv4Cidr> {
    iface
        .get_ip_address()
        .into_iter()
        .find_map(|cidr| match cidr {
            IpCidr::Ipv4(cidr) => {
                let addr = cidr.address();
                if addr.is_unspecified() || cidr.prefix_len() == 0 || is_ipv4_placeholder(addr) {
                    None
                } else {
                    Some(cidr)
                }
            }
            _ => None,
        })
}

pub fn ipv4_sockaddr(addr: Ipv4Address) -> SockAddrIn {
    SockAddrIn {
        sin_family: AddressFamily::Internet.into(),
        sin_port: 0,
        sin_addr: u32::from_ne_bytes(addr.0),
        sin_zero: [0; 8],
    }
}

pub fn ipv4_netmask(prefix_len: u8) -> Ipv4Address {
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - prefix_len as u32)
    };
    Ipv4Address::from_bytes(&mask.to_be_bytes())
}

pub fn ipv4_broadcast(addr: Ipv4Address, prefix_len: u8) -> Ipv4Address {
    let addr_u32 = u32::from_be_bytes(addr.0);
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - prefix_len as u32)
    };
    let broadcast = addr_u32 | !mask;
    Ipv4Address::from_bytes(&broadcast.to_be_bytes())
}

pub fn prefix_len_from_netmask(addr: Ipv4Address) -> LxResult<u8> {
    let mask = u32::from_be_bytes(addr.0);
    let prefix_len = mask.leading_ones() as u8;
    let canonical = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - prefix_len as u32)
    };
    if mask != canonical {
        return Err(LxError::EINVAL);
    }
    Ok(prefix_len)
}

/// missing documentation
pub mod tcp;
pub use tcp::*;

/// missing documentation
pub mod udp;
pub use udp::*;

/// missing documentation
pub mod raw;
pub use raw::*;

/// missing documentation
pub mod packet;
pub use packet::*;

/// missing documentation
pub mod netlink;
pub use netlink::*;
pub mod unix;
pub use unix::*;
pub mod listen_table;
pub use listen_table::*;

/// missing documentation
pub mod icmp;
pub use icmp::*;

/// IPv4 → MAC cache (fed from RX frames).
pub mod arp_cache;
pub use arp_cache::*;

/// IPv6 → MAC cache (fed from RX frames).
pub mod ndp_cache;

/// ICMP echo replies from `push_packet` (ping RX).
pub mod icmp_rx;

/// IPv6 Router Advertisement handling (default route + SLAAC), fed from RX.
pub mod ra;

pub mod dns;

pub mod wait;
pub use icmp_rx::*;

// pub mod stack;

// ============= Socket Set =============
use zcore_drivers::net::get_sockets;
// lazy_static! {
//     /// Global SocketSet in smoltcp.
//     ///
//     /// Because smoltcp is a single thread network stack,
//     /// every socket operation needs to lock this.
//     pub static ref SOCKETS: Mutex<SocketSet<'static>> =
//         Mutex::new(SocketSet::new(vec![]));
// }

// ============= Socket Set =============

// ============= Define =============

// ========TCP

/// TCP send buffer: 512 KiB — large enough that a single window fits a typical
/// APK download chunk even at 100 ms RTT (theoretical limit ~5 MB/s vs ~640 KB/s
/// with the old 64 KiB window).
///
/// MUST stay modest: the kernel heap is a FIXED 256 MiB and every TCP socket
/// pins SENDBUF+RECVBUF up front. apk fetch opens ~30 parallel connections, so
/// at the previous 2 MiB (×2 = 4 MiB/socket) ~30 sockets alone reserved ~120 MiB
/// and a large download exhausted the heap mid-transfer -> kernel OOM panic.
/// That OOM is what surfaced on real hardware as the deterministic ~16-27 MB
/// download "stall" (apk could no longer allocate, stopped reading, the TCP
/// window closed, and the peer went into zero-window persist). 512 KiB ×2 keeps
/// ~30 sockets near ~30 MiB with ample headroom.
pub const TCP_SENDBUF: usize = 512 * 1024;
/// TCP receive buffer: 512 KiB — advertised window size to remote peers.
/// Keeping this equal to SENDBUF avoids asymmetric flow stalls.
pub const TCP_RECVBUF: usize = 512 * 1024;

// ========UDP

/// missing documentation
pub const UDP_METADATA_BUF: usize = 256;
/// missing documentation
pub const UDP_SENDBUF: usize = 64 * 1024;
/// missing documentation
pub const UDP_RECVBUF: usize = 64 * 1024;

/// Largest single kernel-heap `Vec` allocation (avoids multi‑MiB smoltcp/socket buffers).
pub const MAX_KERNEL_VEC: usize = 4 * 1024 * 1024;

/// Allocate a zeroed buffer on the kernel heap, capped and fallible.
pub fn kernel_vec_zeroed(len: usize) -> crate::error::LxResult<alloc::vec::Vec<u8>> {
    if len > MAX_KERNEL_VEC {
        return Err(crate::error::LxError::ENOMEM);
    }
    let mut v = alloc::vec::Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| crate::error::LxError::ENOMEM)?;
    v.resize(len, 0);
    Ok(v)
}

// ========RAW

/// missing documentation
pub const RAW_METADATA_BUF: usize = 64;
/// missing documentation
pub const RAW_SENDBUF: usize = 64 * 1024; // 64K
/// missing documentation
pub const RAW_RECVBUF: usize = 64 * 1024; // 64K

// ========RAW

/// missing documentation
pub const ICMP_METADATA_BUF: usize = 1024;
/// missing documentation
pub const ICMP_SENDBUF: usize = 64 * 1024; // 64K
/// missing documentation
pub const ICMP_RECVBUF: usize = 64 * 1024; // 64K

// ========Other

/// missing documentation
pub const IPPROTO_IP: usize = 0;
/// missing documentation
pub const IP_HDRINCL: usize = 3;

pub const SOCKET_TYPE_MASK: usize = 0xff;

pub const SOCKET_FD: usize = 1000;
pub const SIOCADDRT: usize = 0x890b;
pub const SIOCDELRT: usize = 0x890c;

pub const SIOCGIFCONF: usize = 0x8912;
pub const SIOCGIFFLAGS: usize = 0x8913;
pub const SIOCSIFFLAGS: usize = 0x8914;
pub const SIOCGIFADDR: usize = 0x8915;
pub const SIOCSIFADDR: usize = 0x8916;
pub const SIOCGIFDSTADDR: usize = 0x8917;
pub const SIOCGIFBRDADDR: usize = 0x8919;
pub const SIOCSIFBRDADDR: usize = 0x891a;
pub const SIOCGIFNETMASK: usize = 0x891b;
pub const SIOCSIFNETMASK: usize = 0x891c;
pub const SIOCGIFMETRIC: usize = 0x891d;
pub const SIOCGIFMTU: usize = 0x8921;
pub const SIOCGIFHWADDR: usize = 0x8927;
pub const SIOCGIFINDEX: usize = 0x8933;
pub const SIOCGIFTXQLEN: usize = 0x8942;
pub const SIOCGIFMAP: usize = 0x8970;
pub const SIOCGARP: usize = 0x8954;
pub const ARPHRD_ETHER: u16 = 1;
pub const ARPHRD_LOOPBACK: u16 = 772;

pub const IFF_UP: u32 = 0x1;
pub const IFF_BROADCAST: u32 = 0x2;
pub const IFF_DEBUG: u32 = 0x4;
pub const IFF_LOOPBACK: u32 = 0x8;
pub const IFF_POINTOPOINT: u32 = 0x10;
pub const IFF_NOTRAILERS: u32 = 0x20;
pub const IFF_RUNNING: u32 = 0x40;
pub const IFF_NOARP: u32 = 0x80;
pub const IFF_PROMISC: u32 = 0x100;
pub const IFF_ALLMULTI: u32 = 0x200;
pub const IFF_MASTER: u32 = 0x400;
pub const IFF_SLAVE: u32 = 0x800;
pub const IFF_MULTICAST: u32 = 0x1000;
pub const IFF_LOWER_UP: u32 = 0x1_0000;
/// Kernel-writable bits mask for SIOCSIFFLAGS (ignored — we accept any write).
pub const IFF_CHANGE_ALL: u32 = 0xFFFF_FFFF;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockAddrHw {
    pub sa_family: u16,
    pub sa_data: [u8; 14],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union IfReqUnion {
    pub addr: SockAddrIn,
    pub hwaddr: SockAddrHw,
    pub ifindex: i32,
    pub ifmtu: i32,
    pub ifmetric: i32,
    pub ifqlen: i32,
    pub flags: i16,
    pub ifru_pad: [u64; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IfReq {
    pub ifr_name: [u8; 16],
    pub ifr_ifru: IfReqUnion,
}

impl IfReq {
    pub fn name(&self) -> &str {
        let len = self
            .ifr_name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.ifr_name.len());
        core::str::from_utf8(&self.ifr_name[..len]).unwrap_or("")
    }
}

pub const RTF_UP: u16 = 0x0001;
pub const RTF_GATEWAY: u16 = 0x0002;
pub const RTF_HOST: u16 = 0x0004;

#[repr(C)]
pub struct RtEntry {
    pub rt_pad1: usize,
    pub rt_dst: SockAddrIn,
    pub rt_gateway: SockAddrIn,
    pub rt_genmask: SockAddrIn,
    pub rt_flags: u16,
    pub rt_pad2: i16,
    pub rt_pad3: usize,
    pub rt_pad4: usize,
    pub rt_metric: i16,
    pub rt_dev: *mut u8,
    pub rt_mtu: usize,
    pub rt_window: usize,
    pub rt_irtt: u16,
}

#[repr(C)]
pub struct In6RtMsg {
    pub rtmsg_dst: [u8; 16],
    pub rtmsg_src: [u8; 16],
    pub rtmsg_gateway: [u8; 16],
    pub rtmsg_type: u32,
    pub rtmsg_dst_len: u16,
    pub rtmsg_src_len: u16,
    pub rtmsg_metric: u32,
    pub rtmsg_info: usize,
    pub rtmsg_flags: u32,
    pub rtmsg_ifindex: i32,
}

#[repr(C)]
pub struct IfConf {
    pub ifc_len: i32,
    pub ifc_buf: usize,
}

use numeric_enum_macro::numeric_enum;

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    #[allow(non_camel_case_types)]
    /// Generic musl socket domain.
    pub enum Domain {
    /// Local communication
    AF_UNIX = 1,
        /// IPv4 Internet protocols
        AF_INET = 2,
        /// IPv6 Internet protocols
        AF_INET6 = 10,
        /// Kernel user interface device
        AF_NETLINK = 16,
    /// Low-level packet interface
    AF_PACKET = 17,
    }
}

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    #[allow(non_camel_case_types)]
    /// Generic musl socket type.
    pub enum SocketType {
        /// Provides sequenced, reliable, two-way, connection-based byte streams.
        /// An out-of-band data transmission mechanism may be supported.
        SOCK_STREAM = 1,
        /// Supports datagrams (connectionless, unreliable messages of a fixed maximum length).
        SOCK_DGRAM = 2,
        /// Provides raw network protocol access.
        SOCK_RAW = 3,
        /// Provides a reliable datagram layer that does not guarantee ordering.
        SOCK_RDM = 4,
        /// Provides a sequenced, reliable, two-way connection-based data
        /// transmission path for datagrams of fixed maximum length;
        /// a consumer is required to read an entire packet with each input system call.
        SOCK_SEQPACKET = 5,
        /// Datagram Congestion Control Protocol socket
        SOCK_DCCP = 6,
        /// Obsolete and should not be used in new programs.
        SOCK_PACKET = 10,
        /// Set O_NONBLOCK flag on the open fd
        SOCK_NONBLOCK = 0x800,
        /// Set FD_CLOEXEC flag on the new fd
        SOCK_CLOEXEC = 0x80000,
    }
}

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    #[allow(non_camel_case_types)]
    // define in include/uapi/linux/in.h
    /// Generic musl socket protocol.
    pub enum Protocol {
        /// Dummy protocol for TCP
        IPPROTO_IP = 0,
        /// Internet Control Message Protocol
        IPPROTO_ICMP = 1,
        /// Transmission Control Protocol
        IPPROTO_TCP = 6,
        /// User Datagram Protocol
        IPPROTO_UDP = 17,
        /// IPv6-in-IPv4 tunnelling
        IPPROTO_IPV6 = 41,
        /// ICMPv6
        IPPROTO_ICMPV6 = 58,
    }
}

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    #[allow(non_camel_case_types)]
    /// Generic musl socket level.
    pub enum Level {
        /// ipproto ip
        IPPROTO_IP = 0,
        /// sol socket
        SOL_SOCKET = 1,
        /// ipproto tcp
        IPPROTO_TCP = 6,
    }
}

#[repr(C)]
pub struct MsgHdr {
    pub msg_name: UserInOutPtr<SockAddr>,
    pub msg_namelen: u32,
    _pad1: u32,
    pub msg_iov: UserInPtr<IoVecOut>,
    pub msg_iovlen: usize,
    pub msg_control: UserInOutPtr<u8>,
    pub msg_controllen: usize,
    pub msg_flags: i32,
    _pad2: i32,
}

impl MsgHdr {
    pub fn set_msg_name_len(&mut self, len: u32) {
        self.msg_namelen = len;
    }
}

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    /// Generic musl socket optname.
    pub enum SolOptname {
        /// reuseaddr
        REUSEADDR = 2,
        /// error
        ERROR = 4,
        /// sndbuf
        SNDBUF = 7,  // 获取发送缓冲区长度
        /// rcvbuf
        RCVBUF = 8,  // 获取接收缓冲区长度
        /// linger
        LINGER = 13,
    }
}

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    /// Generic musl socket optname.
    pub enum TcpOptname {
        /// congestion
        CONGESTION = 13,
    }
}

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    /// Generic musl socket optname.
    pub enum IpOptname {
        /// hdrincl
        HDRINCL = 3,
    }
}

// ============= Define =============

// ============= SocketHandle =============

use smoltcp::socket::SocketHandle;

/// Maximum smoltcp sockets (each owns RX/TX buffers on the kernel heap).
pub(super) const MAX_SMOLTCIP_SOCKETS: usize = 96;

pub(super) fn smoltcp_socket_count(set: &smoltcp::socket::SocketSet<'_>) -> usize {
    set.iter().count()
}

/// Register a socket in the global smoltcp set, or return `ENOMEM` if at cap.
pub(super) fn register_smoltcp_socket<T>(socket: T) -> LxResult<GlobalSocketHandle>
where
    T: Into<smoltcp::socket::Socket<'static>>,
{
    let sockets_arc = get_sockets();
    let mut sockets = sockets_arc.lock();
    if smoltcp_socket_count(&sockets) >= MAX_SMOLTCIP_SOCKETS {
        return Err(LxError::ENOMEM);
    }
    Ok(GlobalSocketHandle(sockets.add(socket)))
}

/// A wrapper for `SocketHandle`.
/// Auto increase and decrease reference count on Clone and Drop.
#[derive(Debug)]
pub(super) struct GlobalSocketHandle(SocketHandle);

impl Clone for GlobalSocketHandle {
    fn clone(&self) -> Self {
        get_sockets().lock().retain(self.0);
        Self(self.0)
    }
}

impl Drop for GlobalSocketHandle {
    fn drop(&mut self) {
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        sockets.release(self.0);
        sockets.prune();

        // send FIN immediately when applicable
        drop(sockets);
        poll_ifaces();
    }
}

use kernel_hal::net::get_net_device;

/// True once DHCP (or static config) assigned a real host IPv4 — not placeholder/0.0.0.0.
pub fn has_usable_ipv4() -> bool {
    use smoltcp::wire::IpCidr;
    get_net_device().iter().any(|dev| {
        dev.get_ip_address().iter().any(|ip| match ip {
            IpCidr::Ipv4(cidr) => {
                let addr = cidr.address();
                cidr.prefix_len() > 0
                    && !addr.is_unspecified()
                    && !is_ipv4_placeholder(addr)
                    && addr.0[0] < 240
            }
            _ => false,
        })
    })
}

/// Resolve the IPv4 default gateway from routes or `.1` on the host subnet.
pub fn ipv4_default_gateway(
    dev: &dyn zcore_drivers::scheme::NetScheme,
) -> Option<smoltcp::wire::Ipv4Address> {
    ipv4_gateway_from_routes(&dev.get_routes())
        .filter(|gw| !gw.is_unspecified())
        .or_else(|| infer_ipv4_gateway(dev))
}

/// Ensure smoltcp has a default route (ICMP/TCP/UDP share this stack).
pub fn prepare_ipv4_stack() {
    if !has_usable_ipv4() {
        return;
    }
    for dev in get_net_device().iter() {
        if dev.get_ifname() != "loopback" {
            ensure_ipv4_default_route(dev.as_ref());
        }
    }
}

/// Poll smoltcp then pull any remaining RX into `push_packet` / `icmp_rx`.
pub fn drain_ipv4_nic(dev: &dyn zcore_drivers::scheme::NetScheme, rounds: usize) {
    for _ in 0..rounds {
        kernel_hal::deferred_job::drain_deferred_jobs();
        poll_netdev(dev);
        netdev_drain_rx(dev);
    }
}

/// Import software ARP/NDP learnings into smoltcp's neighbor cache (TCP/UDP egress).
pub fn sync_neighbor_cache_into_smoltcp() {
    use smoltcp::wire::IpAddress;
    for dev in get_net_device().iter() {
        for (ip, mac) in arp_cache::get_entries() {
            let _ = dev.seed_neighbor(IpAddress::Ipv4(ip), mac);
        }
        for (ip, mac) in ndp_cache::get_entries() {
            let _ = dev.seed_neighbor(IpAddress::Ipv6(ip), mac);
        }
    }
}

/// Local UDP bind address for a query toward `dst` (port 0 → ephemeral on send).
pub fn local_udp_endpoint_for(dst: smoltcp::wire::IpAddress) -> IpEndpoint {
    use smoltcp::wire::IpAddress;
    let addr = match dst {
        IpAddress::Ipv4(d) => IpAddress::Ipv4(select_ipv4_for_dst(d)),
        IpAddress::Ipv6(d) => IpAddress::Ipv6(select_ipv6_for_dst(d)),
        other => other,
    };
    IpEndpoint::new(addr, 0)
}

mod adaptive;

/// Minimum interval for heavy net housekeeping (route/neighbor/prune).
const NET_HOUSEKEEPING_INTERVAL_US: u64 = 250_000;

/// Deferred IRQ drain when epoll/poll is not waiting on sockets.
const DEFERRED_IDLE_INTERVAL_US: u64 = 20_000;

static LAST_NET_POLL_US: AtomicU64 = AtomicU64::new(0);
static LAST_NET_HOUSEKEEPING_US: AtomicU64 = AtomicU64::new(0);
static LAST_DEFERRED_IDLE_US: AtomicU64 = AtomicU64::new(0);

/// True for socket / packet / netlink fds (see [`SOCKET_FD`]).
#[inline]
pub fn fd_is_socket(fd: FileDesc) -> bool {
    usize::from(fd) >= SOCKET_FD
}

/// True for stdin, TTY, `/dev/input/*`, etc. (prioritize HID over NIC in wait loops).
#[inline]
pub fn fd_is_interactive(fd: FileDesc) -> bool {
    let raw: i32 = fd.into();
    (raw as usize) < SOCKET_FD && raw >= 0
}

#[inline]
fn mono_us() -> u64 {
    kernel_hal::timer::timer_now().as_micros() as u64
}

fn net_poll_interval_elapsed(interval_us: u64) -> bool {
    let now = mono_us();
    let last = LAST_NET_POLL_US.load(Ordering::Relaxed);
    if now.wrapping_sub(last) < interval_us {
        return false;
    }
    LAST_NET_POLL_US.store(now, Ordering::Relaxed);
    true
}

#[inline]
fn adaptive_deferred_jobs_per_tick() -> usize {
    adaptive::deferred_jobs_budget(kernel_hal::deferred_job::pending_deferred_jobs())
}

#[inline]
fn adaptive_net_poll_interval_us() -> u64 {
    adaptive::net_poll_interval_us(kernel_hal::deferred_job::pending_deferred_jobs())
}

#[inline]
fn maybe_run_net_housekeeping(now_us: u64, force: bool) {
    let last = LAST_NET_HOUSEKEEPING_US.load(Ordering::Relaxed);
    if !force && now_us.wrapping_sub(last) < NET_HOUSEKEEPING_INTERVAL_US {
        return;
    }
    LAST_NET_HOUSEKEEPING_US.store(now_us, Ordering::Relaxed);
    sync_neighbor_cache_into_smoltcp();
    if has_usable_ipv4() {
        prepare_ipv4_stack();
    }
    if let Some(mut sockets) = zcore_drivers::net::get_sockets().try_lock() {
        sockets.prune();
    }
}

/// Throttled NIC poll for multiplex wait loops (epoll/poll/select).
pub fn poll_ifaces_throttled() {
    if !net_poll_interval_elapsed(adaptive_net_poll_interval_us()) {
        return;
    }
    poll_ifaces();
}

/// Socket path: deferred IRQ work + throttled NIC poll.
pub fn drain_net_tick() {
    kernel_hal::deferred_job::drain_deferred_jobs_max(adaptive_deferred_jobs_per_tick());
    poll_ifaces_throttled();
}

/// After recv/send or connect: always run a full NIC poll once.
pub fn drain_net_urgent() {
    kernel_hal::deferred_job::drain_deferred_jobs_max(adaptive_deferred_jobs_per_tick());
    poll_ifaces();
}

/// Register wakers for poll/epoll/select so MSI/TTY IRQs resume the task before timers.
pub fn register_io_wait_wakers(
    waker: &core::task::Waker,
    watch_net: bool,
    watch_interactive: bool,
) {
    if watch_net {
        kernel_hal::net::register_net_rx_waker(waker.clone());
    }
    if watch_interactive {
        crate::fs::stdio::register_tty_intr_waker(waker.clone());
    }
}

/// Called on the poll after an IRQ or timer wake (keep registrations).
pub fn retain_io_wait_wakers(waker: &core::task::Waker, watch_net: bool, watch_interactive: bool) {
    if watch_net {
        kernel_hal::net::retain_net_rx_waker(waker);
    }
    if watch_interactive {
        crate::fs::stdio::retain_tty_intr_waker(waker);
    }
}

/// Remove this wait's waker from IRQ lists when the wait future completes or is
/// dropped. Without this, epoll/poll re-arm leaves stale entries that a later
/// IRQ can fire into after the wait cycle finished.
pub fn clear_io_wait_wakers(waker: &core::task::Waker, watch_net: bool, watch_interactive: bool) {
    if watch_net {
        kernel_hal::net::clear_net_rx_waker(waker);
    }
    if watch_interactive {
        crate::fs::stdio::clear_tty_intr_waker(waker);
    }
}

/// One tick of an I/O wait loop: HID/USB first, then throttled NIC / deferred work.
///
/// When `watch_interactive` and not `watch_net`, skips [`poll_ifaces`] so shell/TTY
/// loops are not blocked by smoltcp/NIC work.
pub fn io_wait_tick(watch_net: bool, _watch_interactive: bool) {
    kernel_hal::input_poll::poll_input_devices();
    if watch_net {
        kernel_hal::deferred_job::drain_deferred_jobs_max(adaptive_deferred_jobs_per_tick());
        poll_ifaces_throttled();
    } else {
        let now = mono_us();
        let last = LAST_DEFERRED_IDLE_US.load(Ordering::Relaxed);
        if now.wrapping_sub(last) >= DEFERRED_IDLE_INTERVAL_US {
            LAST_DEFERRED_IDLE_US.store(now, Ordering::Relaxed);
            kernel_hal::deferred_job::drain_deferred_jobs_max(1);
        }
    }
}

/// Set once networking is up enough to be polled safely (after the first
/// normal `poll_ifaces`, which happens during DHCP — long before any large
/// disk transfer). The AHCI driver's wait loop consults this via
/// `drivers_net_drain` so it never pokes the net stack during early-boot root
/// mounting, before the interfaces/socket set exist.
static NET_DRAIN_READY: AtomicBool = AtomicBool::new(false);
/// Re-entrancy guard: `drivers_net_drain` must never recurse into itself (it
/// won't in practice — the net path issues no disk I/O — but make it
/// structurally impossible).
static NET_DRAINING: AtomicBool = AtomicBool::new(false);

/// Hook called by polled block drivers (AHCI) from inside their command-wait
/// busy loop on real hardware, where a single command/flush can hold the CPU
/// (interrupts disabled, under the fs lock) for long enough to starve the
/// *polled* network stack — overflowing the NIC RX ring and stalling an
/// in-flight TLS download (the "failed to extract libLLVM.so: I/O error" seen
/// only on real hardware, only for the largest package). Servicing the network
/// here keeps the connection alive across long disk waits.
///
/// Safe to call with the fs/AHCI locks held: it only touches the network locks
/// (disjoint), the network never issues disk I/O (no deadlock cycle), and it is
/// a no-op until networking is ready and whenever already draining.
#[no_mangle]
pub extern "C" fn drivers_net_drain() {
    if !NET_DRAIN_READY.load(Ordering::Relaxed) {
        return;
    }
    if NET_DRAINING.swap(true, Ordering::Acquire) {
        return; // already draining on this stack
    }
    poll_ifaces();
    NET_DRAINING.store(false, Ordering::Release);
}

/// miss doc
pub fn poll_ifaces() {
    NET_DRAIN_READY.store(true, Ordering::Relaxed);
    for iface in get_net_device().iter() {
        match iface.poll() {
            Ok(_) => {}
            Err(e) => {
                warn!("error : {:?}", e)
            }
        }
    }
    maybe_run_net_housekeeping(mono_us(), false);
}

/// Poll a single NIC once (smoltcp path — can hold the global socket set lock).
pub fn poll_netdev(dev: &dyn zcore_drivers::scheme::NetScheme) {
    kernel_hal::deferred_job::drain_deferred_jobs();
    let _ = dev.poll();
    kernel_hal::deferred_job::drain_deferred_jobs();
}

/// Pull RX from every non-loopback NIC into `push_packet` / `icmp_rx` (no throttle).
pub fn drain_all_nic_rx() {
    kernel_hal::deferred_job::drain_deferred_jobs();
    for dev in get_net_device().iter() {
        if dev.get_ifname() != "loopback" {
            netdev_drain_rx(dev.as_ref());
        }
    }
}

/// Pull RX frames from the NIC and feed `push_packet` / `icmp_rx` without smoltcp.
pub fn netdev_drain_rx(dev: &dyn zcore_drivers::scheme::NetScheme) {
    // Heap scratch: this runs under epoll/poll `io_wait_tick` on the coroutine
    // stack. A 2 KiB local there nested under DRM/unix wait frames during
    // desktop bring-up.
    let mut buf = alloc::vec![0u8; 2048];
    for _ in 0..32 {
        match dev.recv(&mut buf) {
            Ok(n) if n > 0 => packet::push_packet(&buf[..n]),
            _ => break,
        }
    }
    kernel_hal::deferred_job::drain_deferred_jobs();
}

/// Drive smoltcp until ARP/TX/RX make progress (needed after raw/icmp sends).
pub fn drain_net_poll(rounds: usize) {
    if has_usable_ipv4() {
        prepare_ipv4_stack();
    }
    for i in 0..rounds {
        if i == 0 {
            drain_net_urgent();
        } else {
            drain_net_tick();
        }
        kernel_hal::deferred_job::drain_deferred_jobs_max(adaptive_deferred_jobs_per_tick());
    }
}

/// Pre-DHCP sentinel (Class E); must never be used for TX or ICMP.
pub fn is_ipv4_placeholder(addr: smoltcp::wire::Ipv4Address) -> bool {
    addr == smoltcp::wire::Ipv4Address::new(240, 0, 0, 0)
}

/// True when `dst` is configured on a local interface (excluding placeholders / Class E).
pub fn is_local_host_ipv4(dst: smoltcp::wire::Ipv4Address) -> bool {
    use smoltcp::wire::IpCidr;
    if dst.is_loopback() || !dst.is_unicast() || dst.0[0] >= 240 {
        return false;
    }
    get_net_device().iter().any(|dev| {
        dev.get_ip_address().iter().any(|ip| match ip {
            IpCidr::Ipv4(cidr) => {
                let addr = cidr.address();
                !addr.is_unspecified()
                    && !is_ipv4_placeholder(addr)
                    && cidr.prefix_len() > 0
                    && addr == dst
            }
            _ => false,
        })
    })
}

/// Pick a concrete IPv4 source for `dst` (skip 0.0.0.0/0 catch-all; prefer longest prefix).
pub fn select_ipv4_for_dst(dst: smoltcp::wire::Ipv4Address) -> smoltcp::wire::Ipv4Address {
    use smoltcp::wire::{IpCidr, Ipv4Address};
    let mut best: Option<(u8, Ipv4Address)> = None;
    for iface in get_net_device().iter() {
        for ip in iface.get_ip_address() {
            if let IpCidr::Ipv4(cidr) = ip {
                let addr = cidr.address();
                if addr.is_unspecified() || cidr.prefix_len() == 0 || is_ipv4_placeholder(addr) {
                    continue;
                }
                if cidr.contains_addr(&dst) && best.is_none_or(|(p, _)| cidr.prefix_len() > p) {
                    best = Some((cidr.prefix_len(), addr));
                }
            }
        }
    }
    if let Some((_, a)) = best {
        return a;
    }
    for iface in get_net_device().iter() {
        for ip in iface.get_ip_address() {
            if let IpCidr::Ipv4(cidr) = ip {
                let addr = cidr.address();
                if !addr.is_unspecified() && cidr.prefix_len() > 0 && !is_ipv4_placeholder(addr) {
                    return addr;
                }
            }
        }
    }
    Ipv4Address::UNSPECIFIED
}

/// Drive iface poll until pending smoltcp egress (ARP + TX) has had time to complete.
pub fn flush_socket_egress() {
    drain_net_poll(128);
}

fn is_ipv4_on_link(
    dev: &dyn zcore_drivers::scheme::NetScheme,
    dst: smoltcp::wire::Ipv4Address,
) -> bool {
    use smoltcp::wire::IpCidr;
    dev.get_ip_address().iter().any(|cidr| match cidr {
        IpCidr::Ipv4(v4) => {
            let addr = v4.address();
            v4.prefix_len() > 0
                && !addr.is_unspecified()
                && !is_ipv4_placeholder(addr)
                && v4.contains_addr(&dst)
        }
        _ => false,
    })
}

fn resolve_ipv4_next_hop(
    dev: &dyn zcore_drivers::scheme::NetScheme,
    dst: smoltcp::wire::Ipv4Address,
) -> smoltcp::wire::Ipv4Address {
    use smoltcp::wire::{IpAddress, IpCidr, Ipv4Address};

    // Same subnet (e.g. 192.168.1.1): ARP the host directly — this is why router ping works.
    if is_ipv4_on_link(dev, dst) {
        return dst;
    }

    // Off-subnet (8.8.8.8, 172.x): never ARP the remote IP; always use the default gateway.
    if let Some(gw) = ipv4_default_gateway(dev) {
        return gw;
    }

    // Fallback: longest-prefix match with sane gateway on default routes.
    let mut best: Option<(u8, Ipv4Address)> = None;
    for route in dev.get_routes() {
        let (prefix, matched) = match route.dst {
            IpCidr::Ipv4(cidr) => (cidr.prefix_len(), cidr.contains_addr(&dst)),
            _ => (0, false),
        };
        if !matched {
            continue;
        }
        let next_hop = match (prefix, route.gateway) {
            (_, Some(IpAddress::Ipv4(gw))) if !gw.is_unspecified() => gw,
            (0, _) => infer_ipv4_gateway(dev).unwrap_or(dst),
            _ => dst,
        };
        if best.is_none_or(|(p, _)| prefix > p) {
            best = Some((prefix, next_hop));
        }
    }

    let hop = best.map_or(dst, |(_, hop)| hop);
    if hop == dst {
        if let Some(gw) = infer_ipv4_gateway(dev) {
            return gw;
        }
    }
    hop
}

/// Guess the default gateway when DHCP/`ip route` did not install one.
fn infer_ipv4_gateway(
    dev: &dyn zcore_drivers::scheme::NetScheme,
) -> Option<smoltcp::wire::Ipv4Address> {
    use smoltcp::wire::{IpCidr, Ipv4Address};
    for ip in dev.get_ip_address() {
        if let IpCidr::Ipv4(cidr) = ip {
            let addr = cidr.address();
            if addr.is_unspecified() || cidr.prefix_len() < 8 || is_ipv4_placeholder(addr) {
                continue;
            }
            let o = addr.0;
            // QEMU user networking (slirp): gateway is 10.0.2.2, not .1.
            if o[0] == 10 && o[1] == 0 && o[2] == 2 {
                return Some(Ipv4Address::new(10, 0, 2, 2));
            }
            let gw1 = Ipv4Address::new(o[0], o[1], o[2], 1);
            let gw2 = Ipv4Address::new(o[0], o[1], o[2], 2);
            if arp_cache::lookup(gw2).is_some() {
                return Some(gw2);
            }
            if arp_cache::lookup(gw1).is_some() {
                return Some(gw1);
            }
            return Some(gw1);
        }
    }
    None
}

fn ipv4_gateway_from_routes(
    routes: &[zcore_drivers::scheme::RouteInfo],
) -> Option<smoltcp::wire::Ipv4Address> {
    use smoltcp::wire::{IpAddress, IpCidr};
    routes
        .iter()
        .find_map(|route| match (route.dst, route.gateway) {
            (IpCidr::Ipv4(cidr), Some(IpAddress::Ipv4(gw)))
                if cidr.prefix_len() == 0 && !gw.is_unspecified() =>
            {
                Some(gw)
            }
            _ => None,
        })
}

/// Install `0.0.0.0/0` via DHCP gateway or inferred gateway on the host subnet.
pub fn ensure_ipv4_default_route(iface: &dyn zcore_drivers::scheme::NetScheme) {
    use smoltcp::wire::{IpAddress, IpCidr, Ipv4Address, Ipv4Cidr};
    let Some(gw) = ipv4_default_gateway(iface) else {
        return;
    };
    if ipv4_gateway_from_routes(&iface.get_routes()) == Some(gw) {
        return;
    }
    let default = IpCidr::Ipv4(Ipv4Cidr::new(Ipv4Address::UNSPECIFIED, 0));
    if iface.add_route(default, Some(IpAddress::Ipv4(gw))).is_ok() {
        info!(
            "[net] inferred default IPv4 route via {} on {}",
            gw,
            iface.get_ifname()
        );
    } else {
        warn!(
            "[net] failed to add default IPv4 route via {} on {}",
            gw,
            iface.get_ifname()
        );
    }
}

/// Send a complete IPv4 datagram on the wire (same path as DHCP `PacketSocket`).
pub fn send_ip_ethernet(ip: &[u8]) -> LxResult {
    use smoltcp::wire::{
        ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, Ipv4Packet,
    };

    if ip.len() < 20 {
        return Err(LxError::EINVAL);
    }
    let pkt = Ipv4Packet::new_checked(ip).map_err(|_| LxError::EINVAL)?;
    let dst = pkt.dst_addr();
    if !dst.is_unicast() {
        return Err(LxError::EINVAL);
    }
    let src = pkt.src_addr();
    if src.is_unspecified() {
        return Err(LxError::EINVAL);
    }

    let dev = netdev_for_ipv4(dst)?;
    if has_usable_ipv4() {
        prepare_ipv4_stack();
    } else {
        ensure_ipv4_default_route(dev.as_ref());
    }
    let arp_target = resolve_ipv4_next_hop(dev.as_ref(), dst);
    if arp_target.is_unspecified() || !arp_target.is_unicast() {
        return Err(LxError::EINVAL);
    }
    let our_mac_eth = dev.get_mac();

    const ARP_TRIES: usize = 4;
    for attempt in 0..ARP_TRIES {
        if let Some(dst_mac) = arp_cache::lookup(arp_target) {
            let mut frame = vec![0u8; 14 + ip.len()];
            let mut eth = EthernetFrame::new_unchecked(&mut frame);
            eth.set_dst_addr(dst_mac);
            eth.set_src_addr(our_mac_eth);
            eth.set_ethertype(smoltcp::wire::EthernetProtocol::Ipv4);
            eth.payload_mut().copy_from_slice(ip);
            dev.send(&frame).map_err(|_| LxError::EIO)?;
            drain_ipv4_nic(dev.as_ref(), 4);
            return Ok(());
        }

        // Broadcast ARP who-has (QEMU gateway answers quickly after DHCP).
        let mut arp_buf = vec![0u8; 14 + 28];
        {
            let mut eth = EthernetFrame::new_unchecked(&mut arp_buf);
            eth.set_dst_addr(EthernetAddress::BROADCAST);
            eth.set_src_addr(our_mac_eth);
            eth.set_ethertype(smoltcp::wire::EthernetProtocol::Arp);
            let repr = ArpRepr::EthernetIpv4 {
                operation: ArpOperation::Request,
                source_hardware_addr: our_mac_eth,
                source_protocol_addr: src,
                target_hardware_addr: EthernetAddress([0; 6]),
                target_protocol_addr: arp_target,
            };
            repr.emit(&mut ArpPacket::new_unchecked(eth.payload_mut()));
        }
        dev.send(&arp_buf).map_err(|_| LxError::EIO)?;
        drain_ipv4_nic(dev.as_ref(), 4);
        if attempt + 1 < ARP_TRIES {
            let deadline = kernel_hal::timer::timer_now() + core::time::Duration::from_millis(25);
            while kernel_hal::timer::timer_now() < deadline {
                drain_ipv4_nic(dev.as_ref(), 2);
                if arp_cache::lookup(arp_target).is_some() {
                    break;
                }
                core::hint::spin_loop();
            }
        }
        if arp_cache::lookup(arp_target).is_some() {
            continue;
        }
        if attempt + 1 == ARP_TRIES {
            break;
        }
    }
    Err(LxError::EINVAL)
}

/// Pick a concrete IPv6 source for `dst` (skip ::/0 catch-all; prefer longest prefix).
pub fn select_ipv6_for_dst(dst: smoltcp::wire::Ipv6Address) -> smoltcp::wire::Ipv6Address {
    use smoltcp::wire::{IpCidr, Ipv6Address};
    let mut best: Option<(u8, Ipv6Address)> = None;
    for iface in get_net_device().iter() {
        for ip in iface.get_ip_address() {
            if let IpCidr::Ipv6(cidr) = ip {
                let addr = cidr.address();
                if addr.is_unspecified() || cidr.prefix_len() == 0 {
                    continue;
                }
                if cidr.contains_addr(&dst) && best.is_none_or(|(p, _)| cidr.prefix_len() > p) {
                    best = Some((cidr.prefix_len(), addr));
                }
            }
        }
    }
    if let Some((_, a)) = best {
        return a;
    }
    for iface in get_net_device().iter() {
        for ip in iface.get_ip_address() {
            if let IpCidr::Ipv6(cidr) = ip {
                let addr = cidr.address();
                if !addr.is_unspecified() && cidr.prefix_len() > 0 {
                    return addr;
                }
            }
        }
    }
    Ipv6Address::UNSPECIFIED
}

fn resolve_ipv6_next_hop(
    dev: &dyn zcore_drivers::scheme::NetScheme,
    dst: smoltcp::wire::Ipv6Address,
) -> smoltcp::wire::Ipv6Address {
    use smoltcp::wire::{IpAddress, IpCidr, Ipv6Address};

    let on_link = dev.get_ip_address().iter().any(|cidr| match cidr {
        IpCidr::Ipv6(v6) => {
            v6.prefix_len() > 0 && !v6.address().is_unspecified() && v6.contains_addr(&dst)
        }
        _ => false,
    });
    if on_link {
        return dst;
    }

    let mut best: Option<(u8, Ipv6Address)> = None;
    for route in dev.get_routes() {
        let (prefix, matched) = match route.dst {
            IpCidr::Ipv6(cidr) => (cidr.prefix_len(), cidr.contains_addr(&dst)),
            _ => (0, false),
        };
        if !matched {
            continue;
        }
        let next_hop = match route.gateway {
            Some(IpAddress::Ipv6(gw)) => gw,
            _ => dst,
        };
        if best.is_none_or(|(p, _)| prefix > p) {
            best = Some((prefix, next_hop));
        }
    }

    best.map_or(dst, |(_, hop)| hop)
}

/// Send a complete IPv6 datagram on the wire.
pub fn send_ip6_ethernet(ip: &[u8]) -> LxResult {
    use smoltcp::wire::{
        EthernetAddress, EthernetFrame, Icmpv6Packet, Icmpv6Repr, IpAddress, IpProtocol,
        Ipv6Packet, Ipv6Repr, NdiscRepr,
    };

    if ip.len() < 40 {
        return Err(LxError::EINVAL);
    }
    let pkt = Ipv6Packet::new_checked(ip).map_err(|_| LxError::EINVAL)?;
    let dst = pkt.dst_addr();
    if dst.is_multicast() {
        // Send directly to the mapped multicast MAC address
        let dev = netdev_for_ipv6(dst)?;
        let our_mac_eth = dev.get_mac();
        let dst_mac = EthernetAddress([
            0x33,
            0x33,
            dst.as_bytes()[12],
            dst.as_bytes()[13],
            dst.as_bytes()[14],
            dst.as_bytes()[15],
        ]);
        let mut frame = vec![0u8; 14 + ip.len()];
        let mut eth = EthernetFrame::new_unchecked(&mut frame);
        eth.set_dst_addr(dst_mac);
        eth.set_src_addr(our_mac_eth);
        eth.set_ethertype(smoltcp::wire::EthernetProtocol::Ipv6);
        eth.payload_mut().copy_from_slice(ip);
        dev.send(&frame).map_err(|_| LxError::EIO)?;
        return Ok(());
    }

    if !dst.is_unicast() {
        return Err(LxError::EINVAL);
    }
    let src = pkt.src_addr();
    if src.is_unspecified() {
        return Err(LxError::EINVAL);
    }

    let dev = netdev_for_ipv6(dst)?;
    let ndp_target = resolve_ipv6_next_hop(dev.as_ref(), dst);
    let our_mac_eth = dev.get_mac();

    const NDP_TRIES: usize = 4;
    for attempt in 0..NDP_TRIES {
        if let Some(dst_mac) = ndp_cache::lookup(ndp_target) {
            let mut frame = vec![0u8; 14 + ip.len()];
            let mut eth = EthernetFrame::new_unchecked(&mut frame);
            eth.set_dst_addr(dst_mac);
            eth.set_src_addr(our_mac_eth);
            eth.set_ethertype(smoltcp::wire::EthernetProtocol::Ipv6);
            eth.payload_mut().copy_from_slice(ip);
            dev.send(&frame).map_err(|_| LxError::EIO)?;
            poll_netdev(dev.as_ref());
            return Ok(());
        }

        // Send Neighbor Solicitation (NDP who-has) to the target's solicited node multicast address
        let solicited_node_ip = ndp_target.solicited_node();
        let dst_mac = EthernetAddress([
            0x33,
            0x33,
            solicited_node_ip.as_bytes()[12],
            solicited_node_ip.as_bytes()[13],
            solicited_node_ip.as_bytes()[14],
            solicited_node_ip.as_bytes()[15],
        ]);

        // Construct NS packet
        let ns_repr = Icmpv6Repr::Ndisc(NdiscRepr::NeighborSolicit {
            target_addr: ndp_target,
            lladdr: Some(our_mac_eth),
        });

        let ip_repr = Ipv6Repr {
            src_addr: src,
            dst_addr: solicited_node_ip,
            next_header: IpProtocol::Icmpv6,
            payload_len: ns_repr.buffer_len(),
            hop_limit: 255,
        };

        let total_len = 14 + ip_repr.buffer_len() + ns_repr.buffer_len();
        let mut ns_buf = vec![0u8; total_len];
        {
            let mut eth = EthernetFrame::new_unchecked(&mut ns_buf);
            eth.set_dst_addr(dst_mac);
            eth.set_src_addr(our_mac_eth);
            eth.set_ethertype(smoltcp::wire::EthernetProtocol::Ipv6);

            let mut ip_packet = Ipv6Packet::new_unchecked(eth.payload_mut());
            ip_repr.emit(&mut ip_packet);

            let mut icmp_packet = Icmpv6Packet::new_unchecked(ip_packet.payload_mut());
            ns_repr.emit(
                &IpAddress::Ipv6(src),
                &IpAddress::Ipv6(solicited_node_ip),
                &mut icmp_packet,
                &smoltcp::phy::ChecksumCapabilities::default(),
            );
        }

        dev.send(&ns_buf).map_err(|_| LxError::EIO)?;
        drain_ipv4_nic(dev.as_ref(), 4);
        if attempt + 1 < NDP_TRIES {
            let deadline = kernel_hal::timer::timer_now() + core::time::Duration::from_millis(25);
            while kernel_hal::timer::timer_now() < deadline {
                drain_ipv4_nic(dev.as_ref(), 2);
                if ndp_cache::lookup(ndp_target).is_some() {
                    break;
                }
                core::hint::spin_loop();
            }
        }
        if ndp_cache::lookup(ndp_target).is_some() {
            continue;
        }
        if attempt + 1 == NDP_TRIES {
            break;
        }
    }
    Err(LxError::EINVAL)
}

// ============= SocketHandle =============

// ============= Rand Port =============

/// Return an unpredictable 64-bit value.
///
/// Previously this returned the constant `10000`, which made every DNS
/// transaction ID identical and the ephemeral-port seed fully predictable — the
/// TXID is the resolver's only anti-spoofing check, so a constant made DNS
/// responses trivially forgeable. We now mix a hardware timestamp (rdtsc on
/// x86_64, the monotonic timer elsewhere) into a persistent SplitMix64 counter,
/// so successive calls are distinct and hard to predict even when the clock is
/// coarse.
pub fn rand() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    // Golden-ratio odd increment (SplitMix64). Persisting the state across calls
    // guarantees distinct outputs even if two calls read the same timestamp.
    static STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
    let ts = timestamp_entropy();
    let mut z = STATE
        .fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
        .wrapping_add(ts);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Best-effort high-resolution entropy for [`rand`].
#[allow(unsafe_code)]
fn timestamp_entropy() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        // SAFETY: `rdtsc` is a plain timestamp read with no memory effects.
        unsafe { core::arch::x86_64::_rdtsc() }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        kernel_hal::timer::timer_now().as_nanos() as u64
    }
}

/// Allocate a local ephemeral port in the dynamic range [49152, 65535].
///
/// Uses an atomic counter, seeded once from [`rand`]: the previous `static mut`
/// read-modify-write was a data race (UB) under SMP and could hand the same port
/// to two concurrent `connect()`s. NOTE: this does not (cannot) consult the
/// smoltcp `SocketSet` for an in-use collision — some callers invoke this while
/// already holding the global SOCKETS lock (e.g. tcp `connect`), so re-locking
/// it here would deadlock. The random seed plus the monotonically advancing
/// counter makes a same-4-tuple collision unlikely in practice.
fn get_ephemeral_port() -> u16 {
    use core::sync::atomic::{AtomicU16, Ordering};
    const LOW: u16 = 49152;
    static EPHEMERAL_PORT: AtomicU16 = AtomicU16::new(0);
    // Seed once on first use (the seed is always in [LOW, 65535], never 0).
    let _ = EPHEMERAL_PORT.compare_exchange(
        0,
        LOW + (rand() % (65536 - LOW as u64)) as u16,
        Ordering::Relaxed,
        Ordering::Relaxed,
    );
    // Atomically advance, wrapping within the dynamic range.
    loop {
        let cur = EPHEMERAL_PORT.load(Ordering::Relaxed);
        let next = if !(LOW..65535).contains(&cur) {
            LOW
        } else {
            cur + 1
        };
        if EPHEMERAL_PORT
            .compare_exchange_weak(cur, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return next;
        }
    }
}

// ============= Rand Port =============
// ============= IOCTL =============

pub fn handle_net_ioctl(
    request: usize,
    arg1: usize,
    _arg2: usize,
    _arg3: usize,
    ipv6: bool,
) -> LxResult<usize> {
    match request {
        // SIOCGIFCONF: get list of interfaces
        SIOCGIFCONF => {
            #[allow(unsafe_code)]
            let ifc = unsafe { &mut *(arg1 as *mut IfConf) };
            if ifc.ifc_len < 0 {
                return Err(LxError::EINVAL);
            }
            let buf_bytes = ifc.ifc_len as usize;
            let req_size = size_of::<IfReq>();

            let ifaces = get_net_device();
            let max = if buf_bytes >= req_size {
                buf_bytes / req_size
            } else {
                0
            };
            let count = core::cmp::min(max, ifaces.len());

            #[allow(unsafe_code)]
            let out = unsafe { core::slice::from_raw_parts_mut(ifc.ifc_buf as *mut u8, buf_bytes) };
            for (i, iface) in ifaces.iter().enumerate().take(count) {
                let mut ifr_name = [0u8; 16];
                let name = iface.get_ifname();
                let n = core::cmp::min(15, name.len());
                ifr_name[..n].copy_from_slice(&name.as_bytes()[..n]);

                let addr = iface_ipv4_cidr(&**iface)
                    .map(|cidr| ipv4_sockaddr(cidr.address()))
                    .unwrap_or_else(|| ipv4_sockaddr(Ipv4Address::UNSPECIFIED));
                let ifr = IfReq {
                    ifr_name,
                    ifr_ifru: IfReqUnion { addr },
                };

                let start = i * req_size;
                let end = start + req_size;
                if end <= out.len() {
                    #[allow(unsafe_code)]
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            &ifr as *const IfReq as *const u8,
                            out[start..end].as_mut_ptr(),
                            req_size,
                        );
                    }
                }
            }

            ifc.ifc_len = (count * req_size) as i32;
            Ok(0)
        }

        SIOCGIFINDEX => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let ifaces = get_net_device();
            for (i, iface) in ifaces.iter().enumerate() {
                if iface.get_ifname() == ifname {
                    ifr.ifr_ifru = IfReqUnion {
                        ifindex: (i + 1) as i32,
                    };
                    return Ok(0);
                }
            }
            Err(LxError::ENODEV)
        }

        // SIOCGIFFLAGS: get interface flags
        SIOCGIFFLAGS => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let mut flags = if ifname == "loopback" {
                IFF_UP | IFF_LOOPBACK | IFF_RUNNING | IFF_NOARP
            } else {
                IFF_UP | IFF_BROADCAST | IFF_MULTICAST
            };
            if ifname != "loopback" {
                let iface = iface_by_name(ifname)?;
                if iface.link_carrier_up() {
                    flags |= IFF_RUNNING | IFF_LOWER_UP;
                }
            }
            ifr.ifr_ifru = IfReqUnion {
                flags: flags as i16,
            };
            Ok(0)
        }

        SIOCSIFFLAGS => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &*(arg1 as *const IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            #[allow(unsafe_code)]
            let new_flags = unsafe { ifr.ifr_ifru.flags } as u32;
            warn!(
                "SIOCSIFFLAGS: {} flags={:#x} (admin {})",
                ifname,
                new_flags,
                if new_flags & IFF_UP != 0 {
                    "up"
                } else {
                    "down"
                }
            );
            // IFF_UP is software state — do not reset I219 PHY/RX on every `ifconfig up`.
            Ok(0)
        }

        SIOCGIFADDR => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let iface = iface_by_name(ifname)?;
            let addr = iface_ipv4_cidr(&*iface)
                .map(|cidr| ipv4_sockaddr(cidr.address()))
                .unwrap_or_else(|| ipv4_sockaddr(Ipv4Address::UNSPECIFIED));
            ifr.ifr_ifru = IfReqUnion { addr };
            Ok(0)
        }

        SIOCSIFADDR => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &*(arg1 as *const IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let iface = iface_by_name(ifname)?;
            #[allow(unsafe_code)]
            let addr =
                unsafe { Ipv4Address::from_bytes(&ifr.ifr_ifru.addr.sin_addr.to_ne_bytes()) };
            let prefix_len = iface_ipv4_cidr(&*iface)
                .map(|cidr| cidr.prefix_len())
                .unwrap_or(24); // default /24 until SIOCSIFNETMASK sets the real prefix
            info!("SIOCSIFADDR: {} -> {}/{}", ifname, addr, prefix_len);
            iface
                .set_ipv4_address(Ipv4Cidr::new(addr, prefix_len))
                .map_err(|_| LxError::EINVAL)?;
            prepare_ipv4_stack();
            Ok(0)
        }

        // SIOCSIFBRDADDR: set broadcast address.
        // The broadcast is fully determined by addr + netmask; we just accept the
        // write silently and return success so udhcpc / ifconfig don't error out.
        SIOCSIFBRDADDR => Ok(0),

        SIOCGIFBRDADDR => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let iface = iface_by_name(ifname)?;
            let addr = iface_ipv4_cidr(&*iface)
                .map(|cidr| ipv4_sockaddr(ipv4_broadcast(cidr.address(), cidr.prefix_len())))
                .unwrap_or_else(|| ipv4_sockaddr(Ipv4Address::UNSPECIFIED));
            ifr.ifr_ifru = IfReqUnion { addr };
            Ok(0)
        }

        // SIOCGIFDSTADDR: get point-to-point destination address. Our
        // interfaces are not point-to-point, so the destination is
        // unspecified (0.0.0.0) — exactly what Linux reports for a plain
        // ethernet link. busybox `ifconfig` probes this for every iface;
        // answering it (rather than ENOTTY) keeps the log clean.
        SIOCGIFDSTADDR => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let _ = iface_by_name(ifname)?;
            ifr.ifr_ifru = IfReqUnion {
                addr: ipv4_sockaddr(Ipv4Address::UNSPECIFIED),
            };
            Ok(0)
        }

        SIOCGIFNETMASK => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let iface = iface_by_name(ifname)?;
            let addr = iface_ipv4_cidr(&*iface)
                .map(|cidr| ipv4_sockaddr(ipv4_netmask(cidr.prefix_len())))
                .unwrap_or_else(|| ipv4_sockaddr(Ipv4Address::UNSPECIFIED));
            ifr.ifr_ifru = IfReqUnion { addr };
            Ok(0)
        }

        SIOCSIFNETMASK => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &*(arg1 as *const IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let iface = iface_by_name(ifname)?;
            #[allow(unsafe_code)]
            let netmask =
                unsafe { Ipv4Address::from_bytes(&ifr.ifr_ifru.addr.sin_addr.to_ne_bytes()) };
            let prefix_len = prefix_len_from_netmask(netmask)?;
            let addr = iface_ipv4_cidr(&*iface)
                .map(|cidr| cidr.address())
                .unwrap_or(Ipv4Address::UNSPECIFIED);
            iface
                .set_ipv4_address(Ipv4Cidr::new(addr, prefix_len))
                .map_err(|_| LxError::EINVAL)?;
            prepare_ipv4_stack();
            Ok(0)
        }

        // SIOCGIFHWADDR: get hardware address
        SIOCGIFHWADDR => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let iface = iface_by_name(ifname)?;
            let mac = iface.get_mac();
            unsafe {
                if ifname == "loopback" {
                    ifr.ifr_ifru.hwaddr.sa_family = ARPHRD_LOOPBACK;
                    ifr.ifr_ifru.hwaddr.sa_data[..6].copy_from_slice(&[0; 6]);
                } else {
                    ifr.ifr_ifru.hwaddr.sa_family = ARPHRD_ETHER;
                    ifr.ifr_ifru.hwaddr.sa_data[..6].copy_from_slice(mac.as_bytes());
                }
            }
            Ok(0)
        }

        SIOCGIFTXQLEN => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            let _ = ifreq_name(&ifr.ifr_name)?;
            ifr.ifr_ifru = IfReqUnion { ifqlen: 1000 };
            Ok(0)
        }

        // SIOCGIFMAP: get the device's hardware parameters (struct ifmap:
        // mem_start/mem_end/base_addr/irq/dma/port). We have no such legacy
        // ISA-style parameters to report, so we zero the whole `ifmap` (which
        // fits in `ifru_pad`, 24 bytes). busybox `ifconfig` queries this for
        // every interface; answering keeps the log free of ENOTTY errors.
        SIOCGIFMAP => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let _ = iface_by_name(ifname)?;
            ifr.ifr_ifru = IfReqUnion {
                ifru_pad: [0u64; 3],
            };
            Ok(0)
        }

        // SIOCGIFMTU: get MTU
        SIOCGIFMTU => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            let ifname = ifreq_name(&ifr.ifr_name)?;
            let iface = iface_by_name(ifname)?;
            ifr.ifr_ifru = IfReqUnion {
                ifmtu: iface.get_mtu() as i32,
            };
            Ok(0)
        }

        // SIOCGIFMETRIC: get metric
        SIOCGIFMETRIC => {
            #[allow(unsafe_code)]
            let ifr = unsafe { &mut *(arg1 as *mut IfReq) };
            ifr.ifr_ifru = IfReqUnion { ifmetric: 0 };
            Ok(0)
        }

        // SIOCADDRT: add route
        SIOCADDRT => {
            if ipv6 {
                #[allow(unsafe_code)]
                let rt = unsafe { &*(arg1 as *const In6RtMsg) };
                use smoltcp::wire::{IpAddress, IpCidr, Ipv6Address, Ipv6Cidr};
                let dst_addr = Ipv6Address::from_bytes(&rt.rtmsg_dst);
                let gateway = if (rt.rtmsg_flags & RTF_GATEWAY as u32) != 0 {
                    let addr = Ipv6Address::from_bytes(&rt.rtmsg_gateway);
                    Some(IpAddress::Ipv6(addr))
                } else {
                    None
                };
                let cidr = IpCidr::Ipv6(Ipv6Cidr::new(dst_addr, rt.rtmsg_dst_len as u8));
                let iface = if rt.rtmsg_ifindex > 0 {
                    iface_by_linux_ifindex(rt.rtmsg_ifindex as u32)?
                } else {
                    iface_by_name("eth0")?
                };
                info!(
                    "SIOCADDRT IPv6: cidr={:?}, gateway={:?}, dev={}",
                    cidr,
                    gateway,
                    iface.get_ifname()
                );
                iface.add_route(cidr, gateway).map_err(|_| LxError::EIO)?;
                Ok(0)
            } else {
                #[allow(unsafe_code)]
                let rt = unsafe { &*(arg1 as *const RtEntry) };
                let gateway = if (rt.rt_flags & RTF_GATEWAY) != 0 {
                    let addr = Ipv4Address::from_bytes(&rt.rt_gateway.sin_addr.to_ne_bytes());
                    Some(IpAddress::Ipv4(addr))
                } else {
                    None
                };
                let dst_addr = Ipv4Address::from_bytes(&rt.rt_dst.sin_addr.to_ne_bytes());
                let genmask = Ipv4Address::from_bytes(&rt.rt_genmask.sin_addr.to_ne_bytes());
                let prefix_len = prefix_len_from_netmask(genmask).unwrap_or(0);
                let cidr = IpCidr::Ipv4(Ipv4Cidr::new(dst_addr, prefix_len));

                let ifname = if !rt.rt_dev.is_null() {
                    #[allow(unsafe_code)]
                    unsafe {
                        from_cstr(rt.rt_dev)
                    }
                } else {
                    "eth0" // default to eth0 if not specified
                };

                info!(
                    "SIOCADDRT: cidr={:?}, gateway={:?}, dev={}",
                    cidr, gateway, ifname
                );
                let iface = iface_by_name(ifname)?;
                iface.add_route(cidr, gateway).map_err(|_| LxError::EIO)?;
                Ok(0)
            }
        }

        // SIOCDELRT: delete route
        SIOCDELRT => {
            if ipv6 {
                #[allow(unsafe_code)]
                let rt = unsafe { &*(arg1 as *const In6RtMsg) };
                use smoltcp::wire::{IpAddress, IpCidr, Ipv6Address, Ipv6Cidr};
                let dst_addr = Ipv6Address::from_bytes(&rt.rtmsg_dst);
                let gateway = if (rt.rtmsg_flags & RTF_GATEWAY as u32) != 0 {
                    let addr = Ipv6Address::from_bytes(&rt.rtmsg_gateway);
                    Some(IpAddress::Ipv6(addr))
                } else {
                    None
                };
                let cidr = IpCidr::Ipv6(Ipv6Cidr::new(dst_addr, rt.rtmsg_dst_len as u8));
                let iface = if rt.rtmsg_ifindex > 0 {
                    iface_by_linux_ifindex(rt.rtmsg_ifindex as u32)?
                } else {
                    iface_by_name("eth0")?
                };
                info!(
                    "SIOCDELRT IPv6: cidr={:?}, gateway={:?}, dev={}",
                    cidr,
                    gateway,
                    iface.get_ifname()
                );
                iface.del_route(cidr, gateway).map_err(|_| LxError::EIO)?;
                Ok(0)
            } else {
                #[allow(unsafe_code)]
                let rt = unsafe { &*(arg1 as *const RtEntry) };
                let gateway = if (rt.rt_flags & RTF_GATEWAY) != 0 {
                    let addr = Ipv4Address::from_bytes(&rt.rt_gateway.sin_addr.to_ne_bytes());
                    Some(IpAddress::Ipv4(addr))
                } else {
                    None
                };
                let dst_addr = Ipv4Address::from_bytes(&rt.rt_dst.sin_addr.to_ne_bytes());
                let genmask = Ipv4Address::from_bytes(&rt.rt_genmask.sin_addr.to_ne_bytes());
                let prefix_len = prefix_len_from_netmask(genmask).unwrap_or(0);
                let cidr = IpCidr::Ipv4(Ipv4Cidr::new(dst_addr, prefix_len));

                let ifname = if !rt.rt_dev.is_null() {
                    #[allow(unsafe_code)]
                    unsafe {
                        from_cstr(rt.rt_dev)
                    }
                } else {
                    "eth0" // default to eth0 if not specified
                };

                info!(
                    "SIOCDELRT: cidr={:?}, gateway={:?}, dev={}",
                    cidr, gateway, ifname
                );
                let iface = iface_by_name(ifname)?;
                iface.del_route(cidr, gateway).map_err(|_| LxError::EIO)?;
                Ok(0)
            }
        }

        // SIOCGARP
        SIOCGARP => Err(LxError::ENOENT),

        _ => Err(LxError::ENOSYS),
    }
}

// ============= IOCTL =============
// ============= Rand Port =============

// ============= Util =============

#[allow(unsafe_code)]
/// # Safety
/// Convert C string to Rust string
pub unsafe fn from_cstr(s: *const u8) -> &'static str {
    use core::{slice, str};
    let len = (0usize..).find(|&i| *s.add(i) == 0).unwrap();
    str::from_utf8(slice::from_raw_parts(s, len)).unwrap()
}

// ============= Util =============

use crate::error::*;
use alloc::boxed::Box;
use alloc::fmt::Debug;
use alloc::sync::Arc;
use async_trait::async_trait;
// use core::ops::{Deref, DerefMut};
/// Common methods that a socket must have
#[async_trait]
pub trait Socket: Send + Sync + Debug + downcast_rs::DowncastSync {
    /// missing documentation
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint);
    /// Receive without consuming buffered data when supported.
    async fn peek(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        self.read(data).await
    }
    /// missing documentation
    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult;
    /// wait for some event (in, out, err) on a fd
    fn poll(&self, _events: PollEvents) -> (bool, bool, bool) {
        // Default implementation: socket type does not support poll().
        // Return (false, false, false) instead of panicking.
        warn!("poll() called on a socket that does not implement it");
        (false, false, false)
    }
    /// missing documentation
    async fn connect(&self, endpoint: Endpoint) -> SysResult;
    /// missing documentation
    fn bind(&self, _endpoint: Endpoint) -> SysResult {
        Err(LxError::EINVAL)
    }
    /// missing documentation
    fn listen(&self) -> SysResult {
        Err(LxError::EINVAL)
    }
    /// missing documentation
    fn shutdown(&self, _howto: usize) -> SysResult {
        Err(LxError::EINVAL)
    }
    /// missing documentation
    async fn accept(&self) -> LxResult<(Arc<dyn FileLike>, Endpoint)> {
        Err(LxError::EINVAL)
    }
    /// missing documentation
    fn endpoint(&self) -> Option<Endpoint> {
        None
    }
    /// missing documentation
    fn remote_endpoint(&self) -> Option<Endpoint> {
        None
    }
    /// PID of the process connected to the other end (`SO_PEERCRED`), if this
    /// socket type tracks it. Only AF_UNIX does; the rest return `None`.
    fn peer_pid(&self) -> Option<i32> {
        None
    }
    /// Queue file descriptors to be received by the peer (`SCM_RIGHTS` ancillary
    /// data over a unix socket). Only AF_UNIX supports it. This is how seatd
    /// hands an opened DRM/input device to a Wayland compositor and how clients
    /// pass shm/dmabuf buffers back.
    fn send_fds(&self, _fds: alloc::vec::Vec<Arc<dyn FileLike>>) -> SysResult {
        Err(LxError::ENOSYS)
    }
    /// Take up to `_max` file descriptors the peer attached via `SCM_RIGHTS`.
    fn recv_fds(&self, _max: usize) -> alloc::vec::Vec<Arc<dyn FileLike>> {
        alloc::vec::Vec::new()
    }
    /// Set a socket option.
    ///
    /// We don't yet plumb most options into the smoltcp sockets, but the common
    /// ones that applications set (apk's HTTPS client sets several before a
    /// download) are well-known and harmless to accept: returning success is the
    /// correct, lenient behaviour — failing them would make clients abort. We
    /// recognise the standard option names so the log stays quiet for them and
    /// only note a genuinely unknown option, instead of crying "unimplemented"
    /// on every ordinary `SO_*`/`TCP_*` call (which was a red herring while
    /// debugging download failures). The data path is unchanged.
    fn setsockopt(&self, level: usize, opt: usize, _data: &[u8]) -> SysResult {
        const SOL_SOCKET: usize = 1;
        const IPPROTO_IP: usize = 0;
        const IPPROTO_TCP: usize = 6;
        let known = match level {
            // SO_REUSEADDR, BROADCAST, SNDBUF, RCVBUF, KEEPALIVE, LINGER,
            // REUSEPORT, RCVTIMEO, SNDTIMEO — the usual setsockopt traffic.
            SOL_SOCKET => matches!(opt, 2 | 6 | 7 | 8 | 9 | 13 | 15 | 20 | 21),
            // TCP_NODELAY, TCP_KEEPIDLE/CNT/INTVL, TCP_CONGESTION.
            IPPROTO_TCP => matches!(opt, 1 | 4 | 5 | 6 | 13),
            // IP_TOS, IP_TTL, IP_HDRINCL, multicast knobs.
            IPPROTO_IP => matches!(opt, 1 | 2 | 3 | 32 | 33 | 35),
            _ => false,
        };
        if !known {
            debug!(
                "setsockopt: accepting unhandled level={} opt={}",
                level, opt
            );
        }
        Ok(0)
    }
    /// missing documentation
    fn ioctl(&self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        warn!("ioctl is unimplemented for this socket");
        Ok(0)
    }
    /// Get Socket recv and send buffer capacity
    fn get_buffer_capacity(&self) -> Option<(usize, usize)> {
        None
    }
    /// Get Socket Type
    fn socket_type(&self) -> Option<SocketType> {
        None
    }
    /// `getsockopt(SO_ERROR)`: return and clear the pending socket error (0 if none).
    fn take_so_error(&self) -> i32 {
        0
    }
    /// Flags for the last `recv`/`recvmsg` (e.g. `MSG_TRUNC`); cleared on take.
    fn take_msg_flags(&self) -> i32 {
        0
    }
}

downcast_rs::impl_downcast!(sync Socket);

/*
bitflags::bitflags! {
    /// Socket flags
    #[derive(Default)]
    struct SocketFlags: usize {
        const SOCK_NONBLOCK = 0x800;
        const SOCK_CLOEXEC = 0x80000;
    }
}

impl From<SocketFlags> for OpenOptions {
    fn from(flags: SocketFlags) -> OpenOptions {
        OpenOptions {
            nonblock: flags.contains(SocketFlags::SOCK_NONBLOCK),
            close_on_exec: flags.contains(SocketFlags::SOCK_CLOEXEC),
        }
    }
}
*/

/// One `/proc/net/{tcp,udp}` address cell: the IPv4 bytes read as a
/// little-endian u32 in hex, a colon, then the port in hex — the exact
/// encoding `netstat`/`ss` parse (127.0.0.1:53 → "0100007F:0035").
/// Non-IPv4 addresses render as unspecified; `/proc/net/tcp` is the
/// IPv4 table on Linux too.
fn proc_net_hex_addr(addr: smoltcp::wire::IpAddress, port: u16) -> alloc::string::String {
    use smoltcp::wire::IpAddress;
    let v4 = match addr {
        IpAddress::Ipv4(a) => u32::from_le_bytes(a.0),
        _ => 0,
    };
    alloc::format!("{:08X}:{:04X}", v4, port)
}

/// Linux numeric TCP state codes from `include/net/tcp_states.h`, keyed by
/// the smoltcp state machine.
fn tcp_state_code(state: smoltcp::socket::TcpState) -> u8 {
    use smoltcp::socket::TcpState;
    match state {
        TcpState::Established => 0x01,
        TcpState::SynSent => 0x02,
        TcpState::SynReceived => 0x03,
        TcpState::FinWait1 => 0x04,
        TcpState::FinWait2 => 0x05,
        TcpState::TimeWait => 0x06,
        TcpState::Closed => 0x07,
        TcpState::CloseWait => 0x08,
        TcpState::LastAck => 0x09,
        TcpState::Listen => 0x0A,
        TcpState::Closing => 0x0B,
    }
}

/// `/proc/net/tcp` rendered from the live smoltcp socket set, in the layout
/// of Documentation/networking/proc_net_tcp.rst. The queue/timer columns are
/// zeros (not tracked at this granularity) and the socket's slot number
/// doubles as the inode column — unique per row, which is all `netstat -t`
/// needs to list connections.
pub fn proc_net_tcp_content() -> alloc::string::String {
    use core::fmt::Write;
    use smoltcp::socket::Socket;
    let mut s = alloc::string::String::from(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
    );
    let sets = get_sockets();
    let sets = sets.lock();
    for (i, socket) in sets.iter().enumerate() {
        if let Socket::Tcp(tcp) = socket {
            let l = tcp.local_endpoint();
            let r = tcp.remote_endpoint();
            let _ = writeln!(
                s,
                "{:4}: {} {} {:02X} 00000000:00000000 00:00000000 00000000     0        0 {} 1 0000000000000000 100 0 0 10 0",
                i,
                proc_net_hex_addr(l.addr, l.port),
                proc_net_hex_addr(r.addr, r.port),
                tcp_state_code(tcp.state()),
                1000 + i,
            );
        }
    }
    s
}

/// `/proc/net/udp` from the live smoltcp socket set. UDP rows carry state 07
/// (TCP_CLOSE) like Linux does for unconnected datagram sockets.
pub fn proc_net_udp_content() -> alloc::string::String {
    use core::fmt::Write;
    use smoltcp::socket::Socket;
    let mut s = alloc::string::String::from(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n",
    );
    let sets = get_sockets();
    let sets = sets.lock();
    for (i, socket) in sets.iter().enumerate() {
        if let Socket::Udp(udp) = socket {
            let l = udp.endpoint();
            let _ = writeln!(
                s,
                "{:4}: {} 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 {} 2 0000000000000000 0",
                i,
                proc_net_hex_addr(l.addr, l.port),
                1000 + i,
            );
        }
    }
    s
}

#[cfg(test)]
mod proc_net_tests {
    use super::*;
    use smoltcp::wire::Ipv4Address;

    #[test]
    fn hex_addr_is_little_endian_ipv4_and_hex_port() {
        // The canonical example from proc_net_tcp.rst: 127.0.0.1:53.
        let addr = smoltcp::wire::IpAddress::Ipv4(Ipv4Address::new(127, 0, 0, 1));
        assert_eq!(proc_net_hex_addr(addr, 53), "0100007F:0035");
        let any = smoltcp::wire::IpAddress::Ipv4(Ipv4Address::UNSPECIFIED);
        assert_eq!(proc_net_hex_addr(any, 0), "00000000:0000");
    }

    #[test]
    fn tcp_state_codes_match_tcp_states_h() {
        use smoltcp::socket::TcpState;
        assert_eq!(tcp_state_code(TcpState::Established), 0x01);
        assert_eq!(tcp_state_code(TcpState::Listen), 0x0A);
        assert_eq!(tcp_state_code(TcpState::TimeWait), 0x06);
        assert_eq!(tcp_state_code(TcpState::Closed), 0x07);
    }
}

/// `/proc/net/unix`: the bound (named + abstract) sockets from the unix
/// registry. Connected-but-unnamed pairs are not registered anywhere, so —
/// unlike Linux — they do not show; the named listeners tools grep for
/// (X11, Wayland, D-Bus paths) all do.
pub fn proc_net_unix_content() -> alloc::string::String {
    use core::fmt::Write;
    let mut s =
        alloc::string::String::from("Num       RefCount Protocol Flags    Type St Inode Path\n");
    for (i, (path, refs, listening)) in unix::registry_snapshot().into_iter().enumerate() {
        // Type 0001 = SOCK_STREAM; St 01 = LISTEN-ish/unconnected.
        let st = if listening { "01" } else { "03" };
        let _ = writeln!(
            s,
            "{:08x}: {:08X} 00000000 00010000 0001 {} {} {}",
            i,
            refs,
            st,
            2000 + i,
            path,
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use smoltcp::wire::Ipv4Address;

    #[test]
    fn ipv4_placeholder_is_class_e_sentinel() {
        assert!(is_ipv4_placeholder(Ipv4Address::new(240, 0, 0, 0)));
        assert!(!is_ipv4_placeholder(Ipv4Address::new(192, 168, 1, 1)));
        assert!(!is_ipv4_placeholder(Ipv4Address::new(0, 0, 0, 0)));
    }

    #[test]
    fn fd_is_socket_uses_socket_base() {
        assert!(!fd_is_socket(FileDesc::from(0)));
        assert!(!fd_is_socket(FileDesc::from(999)));
        assert!(fd_is_socket(FileDesc::from(SOCKET_FD as i32)));
        assert!(fd_is_socket(FileDesc::from((SOCKET_FD + 42) as i32)));
    }

    #[test]
    fn fd_is_interactive_covers_stdin_not_sockets() {
        assert!(fd_is_interactive(FileDesc::from(0)));
        assert!(fd_is_interactive(FileDesc::from(2)));
        assert!(!fd_is_interactive(FileDesc::from(SOCKET_FD as i32)));
        assert!(!fd_is_interactive(FileDesc::from(-1)));
    }
}

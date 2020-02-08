#![allow(unsafe_code, dead_code, missing_docs)]

use crate::error::*;
use alloc::boxed::Box;
use alloc::fmt::Debug;
use alloc::sync::Arc;
use alloc::vec::Vec;
use async_trait::async_trait;
use bitflags::*;
use core::future::Future;
use core::mem::size_of;
use core::pin::Pin;
use core::task::{Context, Poll};
use lazy_static::lazy_static;
use numeric_enum_macro::numeric_enum;
use smoltcp::iface::{EthernetInterface, EthernetInterfaceBuilder};
use smoltcp::phy::Device;
use smoltcp::socket::{AnySocket, SocketHandle, SocketRef, SocketSet};
use smoltcp::time::Instant;
pub use smoltcp::wire;
use smoltcp::wire::*;
use spin::Mutex;

//mod tcp;
//mod udp;
//mod raw;

#[derive(Clone, Debug)]
pub struct LinkLevelEndpoint {
    pub interface_index: usize,
}

impl LinkLevelEndpoint {
    pub fn new(ifindex: usize) -> Self {
        LinkLevelEndpoint {
            interface_index: ifindex,
        }
    }
}

#[derive(Clone, Debug)]
pub struct NetlinkEndpoint {
    pub port_id: u32,
    pub multicast_groups_mask: u32,
}

impl NetlinkEndpoint {
    pub fn new(port_id: u32, multicast_groups_mask: u32) -> Self {
        NetlinkEndpoint {
            port_id,
            multicast_groups_mask,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Endpoint {
    Ip(IpEndpoint),
    LinkLevel(LinkLevelEndpoint),
    Netlink(NetlinkEndpoint),
}

/// Common methods that a socket must have
#[async_trait]
pub trait Socket: Send + Sync + Debug {
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint);
    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult;
    fn poll(&self) -> (bool, bool, bool); // (in, out, err)
    async fn connect(&self, endpoint: Endpoint) -> SysResult;
    fn bind(&self, _endpoint: Endpoint) -> SysResult {
        Err(LxError::EINVAL)
    }
    fn listen(&self) -> SysResult {
        Err(LxError::EINVAL)
    }
    fn shutdown(&self) -> SysResult {
        Err(LxError::EINVAL)
    }
    async fn accept(&self) -> LxResult<(Arc<dyn Socket>, Endpoint)> {
        Err(LxError::EINVAL)
    }
    fn endpoint(&self) -> Option<Endpoint> {
        None
    }
    fn remote_endpoint(&self) -> Option<Endpoint> {
        None
    }
    fn setsockopt(&self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        warn!("setsockopt is unimplemented");
        Ok(0)
    }
    fn ioctl(&self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        warn!("ioctl is unimplemented for this socket");
        Ok(0)
    }
}

lazy_static! {
    /// Global SocketSet in smoltcp.
    ///
    /// Because smoltcp is a single thread network stack,
    /// every socket operation needs to lock this.
    ///
    /// TODO: remove from global
    pub static ref SOCKETS: Mutex<SocketSet<'static>> =
        Mutex::new(SocketSet::new(vec![]));
}

/// A wrapper for `SocketHandle`.
/// Auto increase and decrease reference count on Clone and Drop.
#[derive(Debug)]
struct GlobalSocketHandle(SocketHandle);

impl Drop for GlobalSocketHandle {
    fn drop(&mut self) {
        let mut sockets = SOCKETS.lock();
        sockets.release(self.0);
        sockets.prune();

        // send FIN immediately when applicable
        drop(sockets);
        poll_ifaces();
    }
}

struct SmoltcpBase<D: for<'d> Device<'d>> {
    iface: EthernetInterface<'static, D>,
    socket_set: SocketSet<'static>,
    current_time: fn() -> Instant,
}

impl<D: for<'d> Device<'d>> SmoltcpBase<D> {
    fn new(dev: D, current_time: fn() -> Instant) -> Self {
        SmoltcpBase {
            iface: EthernetInterfaceBuilder::new(dev).finalize(),
            socket_set: SocketSet::new(vec![]),
            current_time,
        }
    }

    fn poll(&mut self) {
        let timestamp = (self.current_time)();
        match self.iface.poll(&mut self.socket_set, timestamp) {
            Ok(_) => {}
            Err(e) => debug!("iface poll error: {:?}", e),
        }
    }

    fn get<T: AnySocket<'static>>(&mut self, handle: SocketHandle) -> SocketRef<T> {
        self.socket_set.get(handle)
    }
}

struct IFaceFuture;

impl Future for IFaceFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        unimplemented!()
    }
}

#[repr(C)]
struct ArpReq {
    arp_pa: SockAddrPlaceholder,
    arp_ha: SockAddrPlaceholder,
    arp_flags: u32,
    arp_netmask: SockAddrPlaceholder,
    arp_dev: [u8; 16],
}

#[repr(C)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

#[repr(C)]
pub struct SockAddrUn {
    pub sun_family: u16,
    pub sun_path: [u8; 108],
}

#[repr(C)]
pub struct SockAddrLl {
    pub sll_family: u16,
    pub sll_protocol: u16,
    pub sll_ifindex: u32,
    pub sll_hatype: u16,
    pub sll_pkttype: u8,
    pub sll_halen: u8,
    pub sll_addr: [u8; 8],
}

#[repr(C)]
pub struct SockAddrNl {
    nl_family: u16,
    nl_pad: u16,
    nl_pid: u32,
    nl_groups: u32,
}

#[repr(C)]
pub union SockAddr {
    pub family: u16,
    pub addr_in: SockAddrIn,
    pub addr_un: SockAddrUn,
    pub addr_ll: SockAddrLl,
    pub addr_nl: SockAddrNl,
    pub addr_ph: SockAddrPlaceholder,
}

#[repr(C)]
pub struct SockAddrPlaceholder {
    pub family: u16,
    pub data: [u8; 14],
}

/// Common structure:
/// | nlmsghdr | ifinfomsg/ifaddrmsg | rtattr | rtattr | rtattr | ... | rtattr
/// All aligned to 4 bytes boundary

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct NetlinkMessageHeader {
    nlmsg_len: u32,                   // length of message including header
    nlmsg_type: u16,                  // message content
    nlmsg_flags: NetlinkMessageFlags, // additional flags
    nlmsg_seq: u32,                   // sequence number
    nlmsg_pid: u32,                   // sending process port id
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct IfaceInfoMsg {
    ifi_family: u16,
    ifi_type: u16,
    ifi_index: u32,
    ifi_flags: u32,
    ifi_change: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct IfaceAddrMsg {
    ifa_family: u8,
    ifa_prefixlen: u8,
    ifa_flags: u8,
    ifa_scope: u8,
    ifa_index: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct RouteAttr {
    rta_len: u16,
    rta_type: u16,
}

bitflags! {
    struct NetlinkMessageFlags : u16 {
        const REQUEST = 0x01;
        const MULTI = 0x02;
        const ACK = 0x04;
        const ECHO = 0x08;
        const DUMP_INTR = 0x10;
        const DUMP_FILTERED = 0x20;
        // GET request
        const ROOT = 0x100;
        const MATCH = 0x200;
        const ATOMIC = 0x400;
        const DUMP = 0x100 | 0x200;
        // NEW request
        const REPLACE = 0x100;
        const EXCL = 0x200;
        const CREATE = 0x400;
        const APPEND = 0x800;
        // DELETE request
        const NONREC = 0x100;
        // ACK message
        const CAPPED = 0x100;
        const ACK_TLVS = 0x200;
    }
}

numeric_enum! {
    #[repr(u16)]
    /// Netlink message types
    pub enum NetlinkMessageType {
        /// Nothing
        Noop = 1,
        /// Error
        Error = 2,
        /// End of a dump
        Done = 3,
        /// Data lost
        Overrun = 4,
        /// New link
        NewLink = 16,
        /// Delete link
        DelLink = 17,
        /// Get link
        GetLink = 18,
        /// Set link
        SetLink = 19,
        /// New addr
        NewAddr = 20,
        /// Delete addr
        DelAddr = 21,
        /// Get addr
        GetAddr = 22,
    }
}

numeric_enum! {
    #[repr(u16)]
    /// Route Attr Types
    pub enum RouteAttrTypes {
        /// Unspecified
        Unspecified = 0,
        /// MAC Address
        Address = 1,
        /// Broadcast
        Broadcast = 2,
        /// Interface name
        Ifname = 3,
        /// MTU
        MTU = 4,
        /// Link
        Link = 5,
    }
}

trait VecExt {
    fn align4(&mut self);
    fn push_ext<T: Sized>(&mut self, data: T);
    fn set_ext<T: Sized>(&mut self, offset: usize, data: T);
}

impl VecExt for Vec<u8> {
    fn align4(&mut self) {
        let len = (self.len() + 3) & !3;
        if len > self.len() {
            self.resize(len, 0);
        }
    }

    fn push_ext<T: Sized>(&mut self, data: T) {
        let bytes =
            unsafe { core::slice::from_raw_parts(&data as *const T as *const u8, size_of::<T>()) };
        for byte in bytes {
            self.push(*byte);
        }
    }

    fn set_ext<T: Sized>(&mut self, offset: usize, data: T) {
        if self.len() < offset + size_of::<T>() {
            self.resize(offset + size_of::<T>(), 0);
        }
        let bytes =
            unsafe { core::slice::from_raw_parts(&data as *const T as *const u8, size_of::<T>()) };
        for i in 0..bytes.len() {
            self[offset + i] = bytes[i];
        }
    }
}

fn get_ephemeral_port() -> u16 {
    // TODO selects non-conflict high port
    static mut EPHEMERAL_PORT: u16 = 0;
    unsafe {
        if EPHEMERAL_PORT == 0 {
            EPHEMERAL_PORT = (49152 + kernel_hal::rand_u64() % (65536 - 49152)) as u16;
        }
        if EPHEMERAL_PORT == 65535 {
            EPHEMERAL_PORT = 49152;
        } else {
            EPHEMERAL_PORT = EPHEMERAL_PORT + 1;
        }
        EPHEMERAL_PORT
    }
}

/// Safety: call this without SOCKETS locked
fn poll_ifaces() {
    unimplemented!()
}

numeric_enum! {
    #[repr(u16)]
    #[derive(Debug)]
    /// Address families
    pub enum AddressFamily {
        /// Unspecified
        Unspecified = 0,
        /// Unix domain sockets
        Unix = 1,
        /// Internet IP Protocol
        Internet = 2,
        /// Netlink
        Netlink = 16,
        /// Packet family
        Packet = 17,
    }
}

const IPPROTO_IP: usize = 0;
const IP_HDRINCL: usize = 3;

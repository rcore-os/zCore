//! Linux socket objects
//!

/// missing documentation
#[macro_use]
pub mod socket_address;
use smoltcp::wire::IpEndpoint;
pub use socket_address::*;

/// missing documentation
pub mod tcp;
pub use tcp::*;

/// missing documentation
pub mod udp;
pub use udp::*;

/// missing documentation
pub mod netlink;
use lock::Mutex;
pub use netlink::*;

/// missing documentation
// pub mod raw;
// pub use raw::*;

/// missing documentation
// pub mod icmp;
// pub use icmp::*;

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

/// missing documentation
pub const TCP_SENDBUF: usize = 512 * 1024; // 512K
/// missing documentation
pub const TCP_RECVBUF: usize = 512 * 1024; // 512K

// ========UDP

/// missing documentation
pub const UDP_METADATA_BUF: usize = 1024;
/// missing documentation
pub const UDP_SENDBUF: usize = 64 * 1024; // 64K
/// missing documentation
pub const UDP_RECVBUF: usize = 64 * 1024; // 64K

// ========RAW

/// missing documentation
pub const RAW_METADATA_BUF: usize = 1024;
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

use numeric_enum_macro::numeric_enum;

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Clone, Copy)]
    #[allow(non_camel_case_types)]
    /// Generic musl socket domain.
    pub enum Domain {
        /// Local communication
        AF_LOCAL = 1,
        /// IPv4 Internet protocols
        AF_INET = 2,
        /// IPv6 Internet protocols
        AF_INET6 = 10,
        /// Kernel user interface device
        AF_NETLINK = 16,
    }
}

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Clone, Copy)]
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
    }
}

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Clone, Copy)]
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
    #[derive(Debug, PartialEq, Clone, Copy)]
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

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Clone, Copy)]
    /// Generic musl socket optname.
    pub enum SolOptname {
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
    #[derive(Debug, PartialEq, Clone, Copy)]
    /// Generic musl socket optname.
    pub enum TcpOptname {
        /// congestion
        CONGESTION = 13,
    }
}

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, PartialEq, Clone, Copy)]
    /// Generic musl socket optname.
    pub enum IpOptname {
        /// hdrincl
        HDRINCL = 3,
    }
}

// ============= Define =============

// ============= SocketHandle =============

use smoltcp::socket::SocketHandle;

/// A wrapper for `SocketHandle`.
/// Auto increase and decrease reference count on Clone and Drop.
#[derive(Debug)]
struct GlobalSocketHandle(SocketHandle);

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

/// miss doc
fn poll_ifaces() {
    for iface in get_net_device().iter() {
        match iface.poll() {
            Ok(_) => {}
            Err(e) => {
                warn!("error : {:?}", e)
            }
        }
    }
}

// ============= SocketHandle =============

// ============= Rand Port =============

/// !!!! need riscv rng
pub fn rand() -> u64 {
    // use core::arch::x86_64::_rdtsc;
    // rdrand is not implemented in QEMU
    // so use rdtsc instead
    10000
}

#[allow(unsafe_code)]
/// missing documentation
fn get_ephemeral_port() -> u16 {
    // TODO selects non-conflict high port
    static mut EPHEMERAL_PORT: u16 = 0;
    unsafe {
        if EPHEMERAL_PORT == 0 {
            EPHEMERAL_PORT = (49152 + rand() % (65536 - 49152)) as u16;
        }
        if EPHEMERAL_PORT == 65535 {
            EPHEMERAL_PORT = 49152;
        } else {
            EPHEMERAL_PORT += 1;
        }
        EPHEMERAL_PORT
    }
}

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
pub trait Socket: Send + Sync + Debug {
    /// missing documentation
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint);
    /// missing documentation
    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult;
    /// missing documentation
    fn poll(&self) -> (bool, bool, bool); // (in, out, err)
    /// missing documentation
    async fn connect(&mut self, endpoint: Endpoint) -> SysResult;
    /// missing documentation
    fn bind(&mut self, _endpoint: Endpoint) -> SysResult {
        Err(LxError::EINVAL)
    }
    /// missing documentation
    fn listen(&mut self) -> SysResult {
        Err(LxError::EINVAL)
    }
    /// missing documentation
    fn shutdown(&self) -> SysResult {
        Err(LxError::EINVAL)
    }
    /// missing documentation
    async fn accept(&mut self) -> LxResult<(Arc<Mutex<dyn Socket>>, Endpoint)> {
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
    /// missing documentation
    fn setsockopt(&mut self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        warn!("setsockopt is unimplemented");
        Ok(0)
    }
    /// missing documentation
    fn ioctl(&self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        warn!("ioctl is unimplemented for this socket");
        Ok(0)
    }
    /// missing documentation
    fn fcntl(&self, _cmd: usize, _arg: usize) -> SysResult {
        warn!("ioctl is unimplemented for this socket");
        Ok(0)
    }
}

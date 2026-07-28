// core

use core::{cmp::min, mem::size_of};

// crate
use crate::error::LxError;
// use crate::net::Endpoint;

// smoltcp
pub use smoltcp::wire::{IpAddress, Ipv4Address, Ipv6Address};

use crate::net::*;
use kernel_hal::user::{UserInOutPtr, UserOutPtr};
// use numeric_enum_macro::numeric_enum;
use super::MsgHdr;

/// missing documentation
#[repr(C)]
#[derive(Clone, Copy)]
pub union SockAddr {
    /// missing documentation
    pub family: u16,
    /// missing documentation
    pub addr_in: SockAddrIn,
    /// missing documentation
    pub addr_in6: SockAddrIn6,
    /// missing documentation
    pub addr_un: SockAddrUn,
    /// missing documentation
    pub addr_ll: SockAddrLl,
    /// missing documentation
    pub addr_nl: SockAddrNl,
    /// missing documentation
    pub addr_ph: SockAddrPlaceholder,
}

/// missing documentation
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SockAddrIn6 {
    pub sin6_family: u16,
    pub sin6_port: u16,
    pub sin6_flowinfo: u32,
    pub sin6_addr: [u8; 16],
    pub sin6_scope_id: u32,
}

/// missing documentation
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SockAddrIn {
    /// missing documentation
    pub sin_family: u16,
    /// missing documentation
    pub sin_port: u16,
    /// missing documentation
    pub sin_addr: u32,
    /// missing documentation
    pub sin_zero: [u8; 8],
}

/// missing documentation
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SockAddrUn {
    /// missing documentation
    pub sun_family: u16,
    /// missing documentation
    pub sun_path: [u8; 108],
}

/// missing documentation
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SockAddrLl {
    /// missing documentation
    pub sll_family: u16,
    /// missing documentation
    pub sll_protocol: u16,
    /// missing documentation
    pub sll_ifindex: u32,
    /// missing documentation
    pub sll_hatype: u16,
    /// missing documentation
    pub sll_pkttype: u8,
    /// missing documentation
    pub sll_halen: u8,
    /// missing documentation
    pub sll_addr: [u8; 8],
}

/// missing documentation
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SockAddrNl {
    nl_family: u16,
    nl_pad: u16,
    nl_pid: u32,
    nl_groups: u32,
}

/// missing documentation
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SockAddrPlaceholder {
    /// missing documentation
    pub family: u16,
    /// missing documentation
    pub data: [u8; 14],
}

// ============= Endpoint =============

use smoltcp::wire::IpEndpoint;

/// missing documentation
#[derive(Clone, Debug)]
pub enum Endpoint {
    /// missing documentation
    Ip(IpEndpoint),
    /// missing documentation
    LinkLevel(LinkLevelEndpoint),
    /// missing documentation
    Netlink(NetlinkEndpoint),
    /// missing documentation
    Unix(alloc::string::String),
}

/// missing documentation
#[derive(Clone, Debug)]
pub struct LinkLevelEndpoint {
    /// missing documentation
    pub interface_index: usize,
    /// Hardware address (e.g. MAC)
    pub addr: [u8; 8],
    /// Hardware address length
    pub halen: u8,
    /// Protocol (host-endian)
    pub protocol: u16,
}

impl LinkLevelEndpoint {
    /// missing documentation
    pub fn new(ifindex: usize) -> Self {
        LinkLevelEndpoint {
            interface_index: ifindex,
            addr: [0; 8],
            halen: 0,
            protocol: 0,
        }
    }
}

/// missing documentation
#[derive(Debug, Clone, Copy)]
pub struct NetlinkEndpoint {
    /// missing documentation
    pub port_id: u32,
    /// missing documentation
    pub multicast_groups_mask: u32,
}

impl NetlinkEndpoint {
    /// missing documentation
    pub fn new(port_id: u32, multicast_groups_mask: u32) -> Self {
        NetlinkEndpoint {
            port_id,
            multicast_groups_mask,
        }
    }
}

// ============= Endpoint =============

impl From<Endpoint> for SockAddr {
    fn from(endpoint: Endpoint) -> Self {
        #[allow(warnings)]
        if let Endpoint::Ip(ip) = endpoint {
            match ip.addr {
                IpAddress::Ipv4(ipv4) => SockAddr {
                    addr_in: SockAddrIn {
                        sin_family: AddressFamily::Internet.into(),
                        sin_port: u16::to_be(ip.port),
                        sin_addr: u32::to_be(u32::from_be_bytes(ipv4.0)),
                        sin_zero: [0; 8],
                    },
                },
                IpAddress::Ipv6(ipv6) => SockAddr {
                    addr_in6: SockAddrIn6 {
                        sin6_family: AddressFamily::Internet6.into(),
                        sin6_port: u16::to_be(ip.port),
                        sin6_flowinfo: 0,
                        sin6_addr: ipv6.0,
                        sin6_scope_id: 0,
                    },
                },
                IpAddress::Unspecified | _ => SockAddr {
                    addr_in: SockAddrIn {
                        sin_family: AddressFamily::Internet.into(),
                        sin_port: u16::to_be(ip.port),
                        sin_addr: 0,
                        sin_zero: [0; 8],
                    },
                },
            }
        } else if let Endpoint::LinkLevel(link_level) = endpoint {
            let mut sll_addr = [0u8; 8];
            sll_addr.copy_from_slice(&link_level.addr);
            let hatype = if link_level.interface_index == 1 {
                ARPHRD_LOOPBACK
            } else {
                ARPHRD_ETHER
            };
            SockAddr {
                addr_ll: SockAddrLl {
                    sll_family: AddressFamily::Packet.into(),
                    sll_protocol: link_level.protocol.to_be(),
                    sll_ifindex: link_level.interface_index as u32,
                    sll_hatype: hatype,
                    sll_pkttype: 0,
                    sll_halen: link_level.halen,
                    sll_addr,
                },
            }
        } else if let Endpoint::Netlink(netlink) = endpoint {
            SockAddr {
                addr_nl: SockAddrNl {
                    nl_family: AddressFamily::Netlink.into(),
                    nl_pad: 0,
                    nl_pid: netlink.port_id,
                    nl_groups: netlink.multicast_groups_mask,
                },
            }
        } else if let Endpoint::Unix(path) = endpoint {
            let mut addr_un = SockAddrUn {
                sun_family: AddressFamily::Unix.into(),
                sun_path: [0; 108],
            };
            let bytes = path.as_bytes();
            let len = min(bytes.len(), 107);
            addr_un.sun_path[..len].copy_from_slice(&bytes[..len]);
            SockAddr { addr_un }
        } else {
            // Fallback: return an unspecified address instead of panicking.
            warn!("[socket_address] From<Endpoint>: unrecognised endpoint variant, returning UNSPECIFIED");
            SockAddr {
                addr_ph: SockAddrPlaceholder {
                    family: 0,
                    data: [0; 14],
                },
            }
        }
    }
}

/// missing documentation
pub fn sockaddr_to_endpoint(addr: SockAddr, len: usize) -> Result<Endpoint, LxError> {
    if len < size_of::<u16>() {
        return Err(LxError::EINVAL);
    }
    // let addr = unsafe { vm.check_read_ptr(addr)? };
    if len < addr.len()? {
        return Err(LxError::EINVAL);
    }
    #[allow(unsafe_code)]
    unsafe {
        match AddressFamily::from(addr.family) {
            AddressFamily::Internet => {
                let port = u16::from_be(addr.addr_in.sin_port);
                let addr = IpAddress::from(Ipv4Address::from_bytes(
                    &u32::from_be(addr.addr_in.sin_addr).to_be_bytes()[..],
                ));
                Ok(Endpoint::Ip((addr, port).into()))
            }
            AddressFamily::Internet6 => {
                let port = u16::from_be(addr.addr_in6.sin6_port);
                let addr = IpAddress::Ipv6(Ipv6Address::from_bytes(&addr.addr_in6.sin6_addr));
                Ok(Endpoint::Ip((addr, port).into()))
            }
            AddressFamily::Packet => {
                let mut endpoint = LinkLevelEndpoint::new(addr.addr_ll.sll_ifindex as usize);
                endpoint.halen = addr.addr_ll.sll_halen;
                endpoint.addr.copy_from_slice(&addr.addr_ll.sll_addr);
                endpoint.protocol = u16::from_be(addr.addr_ll.sll_protocol);
                Ok(Endpoint::LinkLevel(endpoint))
            }
            AddressFamily::Netlink => Ok(Endpoint::Netlink(NetlinkEndpoint::new(
                addr.addr_nl.nl_pid,
                addr.addr_nl.nl_groups,
            ))),
            AddressFamily::Unix => {
                let path = {
                    // Number of `sun_path` bytes the caller actually supplied.
                    let avail = if len > 2 {
                        core::cmp::min(len - 2, 108)
                    } else {
                        0
                    };
                    let sun_path = &addr.addr_un.sun_path[..avail];
                    if avail == 0 {
                        // Autobind / unnamed socket.
                        alloc::string::String::new()
                    } else if sun_path[0] == 0 {
                        // Abstract-namespace socket: `sun_path[0]` is NUL and the
                        // name is the remaining `avail - 1` bytes (which are NOT
                        // NUL-terminated — their length comes from `addrlen`).
                        // Represent it with a leading NUL so it can never collide
                        // with a filesystem path (which never starts with NUL).
                        // X11 connects to `\0/tmp/.X11-unix/X0` by default, so
                        // getting this right is what lets `startx` reach the
                        // server.
                        let name = &sun_path[1..];
                        let mut s = alloc::string::String::from("\0");
                        s.push_str(&alloc::string::String::from_utf8_lossy(name));
                        s
                    } else {
                        // Pathname socket: NUL-terminated within `sun_path`.
                        let actual_len = sun_path
                            .iter()
                            .position(|&b| b == 0)
                            .unwrap_or(sun_path.len());
                        alloc::string::String::from_utf8_lossy(&sun_path[..actual_len]).into_owned()
                    }
                };
                Ok(Endpoint::Unix(path))
            }
            _ => Err(LxError::EINVAL),
        }
    }
}

impl SockAddr {
    fn len(&self) -> Result<usize, LxError> {
        #[allow(unsafe_code)]
        match AddressFamily::from(unsafe { self.family }) {
            AddressFamily::Internet => Ok(size_of::<SockAddrIn>()),
            AddressFamily::Internet6 => Ok(size_of::<SockAddrIn6>()),
            AddressFamily::Packet => Ok(size_of::<SockAddrLl>()),
            AddressFamily::Netlink => Ok(size_of::<SockAddrNl>()),
            // Input-validation minimum: Linux accepts a `sockaddr_un` as short as
            // `sizeof(sa_family_t)` (2). For copying an address OUT, use
            // `output_len()` instead so the path is not dropped.
            AddressFamily::Unix => Ok(2),
            _ => Err(LxError::EINVAL),
        }
    }

    /// Serialized length for copying an address OUT to userspace
    /// (getsockname/getpeername/accept/recvfrom). Same as [`len`](Self::len) for
    /// fixed-size families, but for AF_UNIX returns `offsetof(sun_path) + path`
    /// so the bound/peer path is not truncated to just the 2-byte family.
    fn output_len(&self) -> Result<usize, LxError> {
        #[allow(unsafe_code)]
        if AddressFamily::from(unsafe { self.family }) == AddressFamily::Unix {
            const SUN_PATH_OFF: usize = 2; // offsetof(struct sockaddr_un, sun_path)
            #[allow(unsafe_code)]
            let sun_path = unsafe { &self.addr_un.sun_path };
            let path_len = if sun_path[0] == 0 {
                // Abstract namespace (leading NUL + name, not NUL-terminated) or
                // an unnamed socket (all zero). Extend to the last non-zero byte.
                match sun_path.iter().rposition(|&b| b != 0) {
                    Some(last) => last + 1,
                    None => 0,
                }
            } else {
                // Pathname socket: NUL-terminated; include the terminating NUL.
                let nul = sun_path
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(sun_path.len());
                (nul + 1).min(sun_path.len())
            };
            Ok(SUN_PATH_OFF + path_len)
        } else {
            self.len()
        }
    }

    /// # Safety
    /// Write to user sockaddr
    /// Check mutability for user
    #[allow(dead_code)]
    pub fn write_to(
        self,
        addr: UserOutPtr<SockAddr>,
        mut addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        // Ignore NULL
        if addr.is_null() {
            return Ok(0);
        }
        let max_addr_len = addr_len.read()? as usize;
        let full_len = self.output_len()?;
        let written_len = min(max_addr_len, full_len);
        if written_len > 0 {
            #[allow(unsafe_code)]
            let source = unsafe {
                core::slice::from_raw_parts(&self as *const SockAddr as *const u8, written_len)
            };
            #[allow(unsafe_code)]
            let mut addr: UserOutPtr<u8> = unsafe { core::mem::transmute(addr) };
            addr.write_array(source)?;
        }
        addr_len.write(full_len as u32)?;
        Ok(0)
    }

    /// # Safety
    /// Write to user sockaddr
    /// Check mutability for user
    #[allow(dead_code)]
    pub fn write_to_inout(
        self,
        addr: UserInOutPtr<SockAddr>,
        mut addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        // Ignore NULL
        if addr.is_null() {
            return Ok(0);
        }

        let max_addr_len = addr_len.read()? as usize;
        let full_len = self.output_len()?;

        let written_len = min(max_addr_len, full_len);
        if written_len > 0 {
            #[allow(unsafe_code)]
            let source = unsafe {
                core::slice::from_raw_parts(&self as *const SockAddr as *const u8, written_len)
            };
            #[allow(unsafe_code)]
            let mut addr: UserOutPtr<u8> = unsafe { core::mem::transmute(addr) };
            addr.write_array(source)?;
        }
        addr_len.write(full_len as u32)?;
        Ok(0)
    }

    pub fn write_to_msg(&self, mut msg: UserInOutPtr<MsgHdr>) -> SysResult {
        if msg.is_null() {
            return Ok(0);
        }
        let mut hdr = msg.read()?;

        let max_addr_len = hdr.msg_namelen as usize;
        let full_len = self.output_len()?;
        let written_len = min(max_addr_len, full_len);
        hdr.set_msg_name_len(full_len as u32);

        if written_len > 0 && !hdr.msg_name.is_null() {
            #[allow(unsafe_code)]
            unsafe {
                let source =
                    core::slice::from_raw_parts(self as *const SockAddr as *const u8, written_len);
                info!(
                    "write_to_msg: family={}, msg_namelen={}, written_len={}, source={:x?}",
                    self.family, hdr.msg_namelen, written_len, source
                );
                // Use transmute_copy to convert UserInOutPtr<SockAddr> to UserOutPtr<u8> for byte-wise writing
                let mut addr_ptr: UserOutPtr<u8> = core::mem::transmute_copy(&hdr.msg_name);
                addr_ptr.write_array(source)?;
            }
        }

        msg.write(hdr)?;
        Ok(0)
    }
}

/// missing documentation
#[macro_export]
macro_rules! enum_with_unknown {
    (
        $( #[$enum_attr:meta] )*
        pub enum $name:ident($ty:ty) {
            $( $variant:ident = $value:expr ),+ $(,)*
        }
    ) => {
        enum_with_unknown! {
            $( #[$enum_attr] )*
            pub doc enum $name($ty) {
                $( #[doc(shown)] $variant = $value ),+
            }
        }
    };
    (
        $( #[$enum_attr:meta] )*
        pub doc enum $name:ident($ty:ty) {
            $(
              $( #[$variant_attr:meta] )+
              $variant:ident = $value:expr $(,)*
            ),+
        }
    ) => {
        #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
        $( #[$enum_attr] )*
        pub enum $name {
            $(
              $( #[$variant_attr] )*
              $variant
            ),*,
            /// missing documentation
            Unknown($ty)
        }

        impl ::core::convert::From<$ty> for $name {
            fn from(value: $ty) -> Self {
                match value {
                    $( $value => $name::$variant ),*,
                    other => $name::Unknown(other)
                }
            }
        }

        impl ::core::convert::From<$name> for $ty {
            fn from(value: $name) -> Self {
                match value {
                    $( $name::$variant => $value ),*,
                    $name::Unknown(other) => other
                }
            }
        }
    }
}

enum_with_unknown! {
    /// Address families
    pub doc enum AddressFamily(u16) {
        /// Unspecified
        Unspecified = 0,
        /// Unix domain sockets
        Unix = 1,
        /// Internet IP Protocol
        Internet = 2,
        /// IPv6 Internet IP Protocol
        Internet6 = 10,
        /// Netlink
        Netlink = 16,
        /// Packet family
        Packet = 17,
    }
}

/// missing documentation
#[repr(C)]
pub struct ArpReq {
    /// missing documentation
    pub arp_pa: SockAddrPlaceholder,
    /// missing documentation
    pub arp_ha: SockAddrPlaceholder,
    /// missing documentation
    pub arp_flags: u32,
    /// missing documentation
    pub arp_netmask: SockAddrPlaceholder,
    /// missing documentation
    pub arp_dev: [u8; 16],
}

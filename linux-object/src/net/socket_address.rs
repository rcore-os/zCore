// core

use core::cmp::min;
use core::mem::size_of;

// crate
use crate::error::LxError;
// use crate::net::Endpoint;

// smoltcp
pub use smoltcp::wire::{IpAddress, Ipv4Address};

use crate::net::*;
use kernel_hal::user::{UserInOutPtr, UserOutPtr};
// use numeric_enum_macro::numeric_enum;

/// missing documentation
#[repr(C)]
pub union SockAddr {
    /// missing documentation
    pub family: u16,
    /// missing documentation
    pub addr_in: SockAddrIn,
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
#[repr(C)]
pub struct SockAddrUn {
    /// missing documentation
    pub sun_family: u16,
    /// missing documentation
    pub sun_path: [u8; 108],
}

/// missing documentation
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
#[repr(C)]
pub struct SockAddrNl {
    nl_family: u16,
    nl_pad: u16,
    nl_pid: u32,
    nl_groups: u32,
}

/// missing documentation
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
}

/// missing documentation
#[derive(Clone, Debug)]
pub struct LinkLevelEndpoint {
    /// missing documentation
    pub interface_index: usize,
}

impl LinkLevelEndpoint {
    /// missing documentation
    pub fn new(ifindex: usize) -> Self {
        LinkLevelEndpoint {
            interface_index: ifindex,
        }
    }
}

/// missing documentation
#[derive(Clone, Debug)]
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
                IpAddress::Unspecified => SockAddr {
                    addr_ph: SockAddrPlaceholder {
                        family: AddressFamily::Unspecified.into(),
                        data: [0; 14],
                    },
                },
                _ => unimplemented!("only ipv4"),
            }
        } else if let Endpoint::LinkLevel(link_level) = endpoint {
            SockAddr {
                addr_ll: SockAddrLl {
                    sll_family: AddressFamily::Packet.into(),
                    sll_protocol: 0,
                    sll_ifindex: link_level.interface_index as u32,
                    sll_hatype: 0,
                    sll_pkttype: 0,
                    sll_halen: 0,
                    sll_addr: [0; 8],
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
        } else {
            unimplemented!("not match");
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
            AddressFamily::Unix => Err(LxError::EINVAL),
            // AddressFamily::Packet => Ok(Endpoint::LinkLevel(LinkLevelEndpoint::new(
            //     addr.addr_ll.sll_ifindex as usize,
            // ))),
            // AddressFamily::Netlink => Ok(Endpoint::Netlink(NetlinkEndpoint::new(
            //     addr.addr_nl.nl_pid,
            //     addr.addr_nl.nl_groups,
            // ))),
            _ => Err(LxError::EINVAL),
        }
    }
}

impl SockAddr {
    fn len(&self) -> Result<usize, LxError> {
        #[allow(unsafe_code)]
        match AddressFamily::from(unsafe { self.family }) {
            AddressFamily::Internet => Ok(size_of::<SockAddrIn>()),
            AddressFamily::Packet => Ok(size_of::<SockAddrLl>()),
            AddressFamily::Netlink => Ok(size_of::<SockAddrNl>()),
            AddressFamily::Unix => Err(LxError::EINVAL),
            _ => Err(LxError::EINVAL),
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
        let full_len = self.len()?;

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
}

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

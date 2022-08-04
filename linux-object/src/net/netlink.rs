// udpsocket

use super::socket_address::*;
use crate::{
    error::{LxError, LxResult},
    fs::FileLike,
    net::{AddressFamily, Endpoint, SockAddr, Socket, SysResult},
};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use async_trait::async_trait;
use bitflags::bitflags;
use core::{mem::size_of, slice};
use kernel_hal::{net::get_net_device, user::*};
use lock::Mutex;

#[derive(Debug, Clone)]
pub struct NetlinkSocketState {
    data: Arc<Mutex<Vec<Vec<u8>>>>,
    _local_endpoint: Option<NetlinkEndpoint>,
}

impl Default for NetlinkSocketState {
    fn default() -> Self {
        Self {
            data: Arc::new(Mutex::new(Vec::new())),
            _local_endpoint: Some(NetlinkEndpoint::new(0, 0)),
        }
    }
}
impl NetlinkSocketState {}

#[async_trait]
impl Socket for NetlinkSocketState {
    /// missing documentation
    async fn read(&self, data: &mut [u8]) -> (LxResult<usize>, Endpoint) {
        let mut buffer = self.data.lock();
        let msg = buffer.remove(0);
        let len = msg.len();
        if !msg.is_empty() {
            data[..len].copy_from_slice(&msg[..len]);
            (
                Ok(len),
                Endpoint::Netlink(NetlinkEndpoint {
                    port_id: 0,
                    multicast_groups_mask: 0,
                }),
            )
        } else {
            (
                Ok(0),
                Endpoint::Netlink(NetlinkEndpoint {
                    port_id: 0,
                    multicast_groups_mask: 0,
                }),
            )
        }
    }

    fn write(&self, data: &[u8], _sendto_endpoint: Option<Endpoint>) -> SysResult {
        if data.len() < size_of::<NetlinkMessageHeader>() {
            return Err(LxError::EINVAL);
        }
        #[allow(unsafe_code)]
        let header = unsafe { &*(data.as_ptr() as *const NetlinkMessageHeader) };
        if header.nlmsg_len as usize > data.len() {
            return Err(LxError::EINVAL);
        }
        let message_type = NetlinkMessageType::from(header.nlmsg_type);
        let mut buffer = self.data.lock();
        buffer.clear();
        match message_type {
            NetlinkMessageType::GetLink => {
                let ifaces = get_net_device();
                for (i, iface) in ifaces.iter().enumerate() {
                    let mut msg = Vec::new();
                    let new_header = NetlinkMessageHeader {
                        nlmsg_len: 0, // to be determined later
                        nlmsg_type: NetlinkMessageType::NewLink.into(),
                        nlmsg_flags: NetlinkMessageFlags::MULTI,
                        nlmsg_seq: header.nlmsg_seq,
                        nlmsg_pid: header.nlmsg_pid,
                    };
                    msg.push_ext(new_header);

                    let if_info = IfaceInfoMsg {
                        ifi_family: AddressFamily::Unspecified.into(),
                        ifi_type: 0,
                        ifi_index: i as u32,
                        ifi_flags: 0,
                        ifi_change: 0,
                    };
                    msg.align4();
                    msg.push_ext(if_info);

                    let mut attrs = Vec::new();

                    let mac_addr = iface.get_mac();
                    let attr = RouteAttr {
                        rta_len: (mac_addr.as_bytes().len() + size_of::<RouteAttr>()) as u16,
                        rta_type: RouteAttrTypes::Address.into(),
                    };
                    attrs.align4();
                    attrs.push_ext(attr);
                    for byte in mac_addr.as_bytes() {
                        attrs.push(*byte);
                    }

                    let ifname = iface.get_ifname();
                    let attr = RouteAttr {
                        rta_len: (ifname.as_bytes().len() + size_of::<RouteAttr>()) as u16,
                        rta_type: RouteAttrTypes::Ifname.into(),
                    };
                    attrs.align4();
                    attrs.push_ext(attr);
                    for byte in ifname.as_bytes() {
                        attrs.push(*byte);
                    }

                    msg.align4();
                    msg.append(&mut attrs);

                    msg.align4();
                    msg.set_ext(0, msg.len() as u32);

                    buffer.push(msg);
                }
            }
            NetlinkMessageType::GetAddr => {
                let ifaces = get_net_device();
                for (i, iface) in ifaces.iter().enumerate() {
                    let ip_addrs = iface.get_ip_address();

                    // for j in 0..ip_addrs.len() {
                    for ip in &ip_addrs {
                        let mut msg = Vec::new();
                        let new_header = NetlinkMessageHeader {
                            nlmsg_len: 0, // to be determined later
                            nlmsg_type: NetlinkMessageType::NewAddr.into(),
                            nlmsg_flags: NetlinkMessageFlags::MULTI,
                            nlmsg_seq: header.nlmsg_seq,
                            nlmsg_pid: header.nlmsg_pid,
                        };
                        msg.push_ext(new_header);

                        let family: u16 = AddressFamily::Internet.into();
                        let if_addr = IfaceAddrMsg {
                            ifa_family: family as u8,
                            ifa_prefixlen: ip.prefix_len(),
                            ifa_flags: 0,
                            ifa_scope: 0,
                            ifa_index: i as u32,
                        };
                        msg.align4();
                        msg.push_ext(if_addr);

                        let mut attrs = Vec::new();

                        let ip_addr = ip.address();
                        let attr = RouteAttr {
                            rta_len: (ip_addr.as_bytes().len() + size_of::<RouteAttr>()) as u16,
                            rta_type: RouteAttrTypes::Address.into(),
                        };
                        attrs.align4();
                        attrs.push_ext(attr);
                        for byte in ip_addr.as_bytes() {
                            attrs.push(*byte);
                        }

                        msg.align4();
                        msg.append(&mut attrs);

                        msg.align4();
                        msg.set_ext(0, msg.len() as u32);

                        buffer.push(msg);
                    }
                }
            }
            _ => {}
        }
        let mut msg = Vec::new();
        let new_header = NetlinkMessageHeader {
            nlmsg_len: 0, // to be determined later
            nlmsg_type: NetlinkMessageType::Done.into(),
            nlmsg_flags: NetlinkMessageFlags::MULTI,
            nlmsg_seq: header.nlmsg_seq,
            nlmsg_pid: header.nlmsg_pid,
        };
        msg.push_ext(new_header);
        msg.align4();
        msg.set_ext(0, msg.len() as u32);
        buffer.push(msg);
        Ok(data.len())
    }

    /// connect
    async fn connect(&self, _endpoint: Endpoint) -> SysResult {
        unimplemented!()
    }

    fn bind(&self, _endpoint: Endpoint) -> SysResult {
        warn!("bind netlink socket");
        // if let Endpoint::Netlink(mut net_link) = endpoint {
        //     if net_link.port_id == 0 {
        //         net_link.port_id = get_ephemeral_port();
        //     }
        //     self.local_endpoint = Some(ip);
        //     self.is_listening = false;
        //     Ok(0)
        // } else {
        //     Err(LxError::EINVAL)
        // }
        Ok(0)
    }

    fn listen(&self) -> SysResult {
        unimplemented!()
    }

    fn shutdown(&self) -> SysResult {
        unimplemented!()
    }

    async fn accept(&self) -> LxResult<(Arc<dyn FileLike>, Endpoint)> {
        unimplemented!()
    }

    fn endpoint(&self) -> Option<Endpoint> {
        Some(Endpoint::Netlink(NetlinkEndpoint::new(0, 0)))
    }

    fn remote_endpoint(&self) -> Option<Endpoint> {
        unimplemented!()
    }

    fn setsockopt(&self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        Ok(0)
    }

    fn ioctl(&self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        Ok(0)
    }
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

enum_with_unknown! {
    /// Netlink message types
    pub doc enum NetlinkMessageType(u16) {
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

enum_with_unknown! {
    /// Route Attr Types
    pub doc enum RouteAttrTypes(u16) {
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
        #[allow(unsafe_code)]
        let bytes =
            unsafe { slice::from_raw_parts(&data as *const T as *const u8, size_of::<T>()) };
        for byte in bytes {
            self.push(*byte);
        }
    }

    fn set_ext<T: Sized>(&mut self, offset: usize, data: T) {
        if self.len() < offset + size_of::<T>() {
            self.resize(offset + size_of::<T>(), 0);
        }
        #[allow(unsafe_code)]
        let bytes =
            unsafe { slice::from_raw_parts(&data as *const T as *const u8, size_of::<T>()) };
        self[offset..(bytes.len() + offset)].copy_from_slice(bytes);
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct MsgHdr {
    pub msg_name: UserInOutPtr<SockAddr>,
    pub msg_namelen: u32,
    pub msg_iov: UserInPtr<IoVecOut>,
    pub msg_iovlen: usize,
    pub msg_control: usize,
    pub msg_controllen: usize,
    pub msg_flags: usize,
}

impl MsgHdr {
    pub fn set_msg_name_len(&mut self, len: u32) {
        self.msg_namelen = len;
    }
}

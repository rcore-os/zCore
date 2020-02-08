//! Syscalls for networking
#![allow(missing_docs)]

use super::*;
use core::cmp::min;
use core::mem::size_of;
use linux_object::net::{wire::*, *};
use numeric_enum_macro::numeric_enum;

impl Syscall<'_> {
    pub fn sys_socket(&mut self, domain: usize, socket_type: usize, protocol: usize) -> SysResult {
        let domain = AddressFamily::try_from(domain as u16).map_err(|_| LxError::EINVAL)?;
        let socket_type = SocketType::try_from(socket_type as u8 & SOCK_TYPE_MASK)
            .map_err(|_| LxError::EINVAL)?;
        info!(
            "socket: domain={:?}, socket_type={:?}, protocol={}",
            domain, socket_type, protocol
        );
        unimplemented!();
        // let proc = self.linux_process();
        // let socket: Arc<dyn Socket> = match domain {
        //     AddressFamily::Internet | AddressFamily::Unix => match socket_type {
        //         SocketType::Stream => Arc::new(TcpSocketState::new()),
        //         SocketType::Datagram => Arc::new(UdpSocketState::new()),
        //         SocketType::Raw => Arc::new(RawSocketState::new(protocol as u8)),
        //     },
        //     AddressFamily::Packet => match socket_type {
        //         SocketType::Raw => Arc::new(PacketSocketState::new()),
        //         _ => return Err(LxError::EINVAL),
        //     },
        //     AddressFamily::Netlink => match socket_type {
        //         SocketType::Raw => Arc::new(NetlinkSocketState::new()),
        //         _ => return Err(LxError::EINVAL),
        //     },
        //     _ => return Err(LxError::EAFNOSUPPORT),
        // };
        // let fd = proc.add_socket(socket)?;
        // Ok(fd.into())
    }

    pub fn sys_setsockopt(
        &mut self,
        fd: FileDesc,
        level: usize,
        optname: usize,
        optval: UserInPtr<u8>,
        optlen: usize,
    ) -> SysResult {
        info!(
            "setsockopt: fd={:?}, level={}, optname={}",
            fd, level, optname
        );
        let proc = self.linux_process();
        let data = optval.read_array(optlen)?;
        let socket = proc.get_socket(fd)?;
        socket.setsockopt(level, optname, &data)
    }

    pub fn sys_getsockopt(
        &mut self,
        fd: FileDesc,
        level: usize,
        optname: usize,
        optval: UserOutPtr<u32>,
        mut optlen: UserOutPtr<u32>,
    ) -> SysResult {
        info!(
            "getsockopt: fd={:?}, level={}, optname={} optval={:?} optlen={:?}",
            fd, level, optname, optval, optlen
        );
        match level {
            SOL_SOCKET => match optname {
                SO_SNDBUF => {
                    //                    optval.write(TCP_SENDBUF as u32)?;
                    optlen.write(4)?;
                    Ok(0)
                }
                SO_RCVBUF => {
                    //                    optval.write(TCP_RECVBUF as u32)?;
                    optlen.write(4)?;
                    Ok(0)
                }
                _ => Err(LxError::ENOPROTOOPT),
            },
            IPPROTO_TCP => match optname {
                TCP_CONGESTION => Ok(0),
                _ => Err(LxError::ENOPROTOOPT),
            },
            _ => Err(LxError::ENOPROTOOPT),
        }
    }

    pub async fn sys_connect(
        &mut self,
        fd: FileDesc,
        addr: UserInPtr<SockAddr>,
        addr_len: usize,
    ) -> SysResult {
        info!(
            "sys_connect: fd={:?}, addr={:?}, addr_len={}",
            fd, addr, addr_len
        );

        let proc = self.linux_process();
        let endpoint = sockaddr_to_endpoint(addr, addr_len)?;
        let socket = proc.get_socket(fd)?;
        socket.connect(endpoint).await?;
        Ok(0)
    }

    pub fn sys_sendto(
        &mut self,
        fd: FileDesc,
        base: UserInPtr<u8>,
        len: usize,
        _flags: usize,
        addr: UserInPtr<SockAddr>,
        addr_len: usize,
    ) -> SysResult {
        info!(
            "sys_sendto: fd={:?} base={:?} len={} addr={:?} addr_len={}",
            fd, base, len, addr, addr_len
        );

        let proc = self.linux_process();

        let slice = base.read_array(len)?;
        let endpoint = if addr.is_null() {
            None
        } else {
            let endpoint = sockaddr_to_endpoint(addr, addr_len)?;
            info!("sys_sendto: sending to endpoint {:?}", endpoint);
            Some(endpoint)
        };
        let socket = proc.get_socket(fd)?;
        socket.write(&slice, endpoint)
    }

    pub async fn sys_recvfrom(
        &mut self,
        fd: FileDesc,
        mut base: UserOutPtr<u8>,
        len: usize,
        flags: usize,
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_recvfrom: fd={:?} base={:?} len={} flags={} addr={:?} addr_len={:?}",
            fd, base, len, flags, addr, addr_len
        );

        let proc = self.linux_process();

        let socket = proc.get_socket(fd)?;
        let mut slice = vec![0u8; len];
        let (result, endpoint) = socket.read(&mut slice).await;
        base.write_array(&slice)?;

        if result.is_ok() && !addr.is_null() {
            let sockaddr_in = SockAddr::from(endpoint);
            sockaddr_in.write_to(addr, addr_len)?;
        }

        result
    }

    //    pub fn sys_recvmsg(&mut self, fd: FileDesc, msg: *mut MsgHdr, flags: usize) -> SysResult {
    //        info!("recvmsg: fd={:?}, msg={:?}, flags={}", fd, msg, flags);
    //        let proc = self.linux_process();
    //        let hdr = unsafe { self.vm().check_write_ptr(msg)? };
    //        let mut iovs =
    //            unsafe { IoVecs::check_and_new(hdr.msg_iov, hdr.msg_iovlen, &self.vm(), true)? };
    //
    //        let mut buf = iovs.new_buf(true);
    //        let socket = proc.get_socket(fd)?;
    //        let (result, endpoint) = socket.read(&mut buf);
    //
    //        if let Ok(len) = result {
    //            // copy data to user
    //            iovs.write_all_from_slice(&buf[..len]);
    //            let sockaddr_in = SockAddr::from(endpoint);
    //            sockaddr_in.write_to(hdr.msg_name, &mut hdr.msg_namelen as *mut u32)?;
    //        }
    //        result
    //    }

    pub fn sys_bind(
        &mut self,
        fd: FileDesc,
        addr: UserInPtr<SockAddr>,
        addr_len: usize,
    ) -> SysResult {
        info!("sys_bind: fd={:?} addr={:?} len={}", fd, addr, addr_len);
        let proc = self.linux_process();

        let endpoint = sockaddr_to_endpoint(addr, addr_len)?;
        info!("sys_bind: fd={:?} bind to {:?}", fd, endpoint);

        let socket = proc.get_socket(fd)?;
        socket.bind(endpoint)
    }

    pub fn sys_listen(&mut self, fd: FileDesc, backlog: usize) -> SysResult {
        info!("sys_listen: fd={:?} backlog={}", fd, backlog);
        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let proc = self.linux_process();

        let socket = proc.get_socket(fd)?;
        socket.listen()
    }

    pub fn sys_shutdown(&mut self, fd: FileDesc, how: usize) -> SysResult {
        info!("sys_shutdown: fd={:?} how={}", fd, how);
        let proc = self.linux_process();

        let socket = proc.get_socket(fd)?;
        socket.shutdown()
    }

    pub async fn sys_accept(
        &mut self,
        fd: FileDesc,
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_accept: fd={:?} addr={:?} addr_len={:?}",
            fd, addr, addr_len
        );
        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let proc = self.linux_process();

        let socket = proc.get_socket(fd)?;
        let (new_socket, remote_endpoint) = socket.accept().await?;

        let new_fd = proc.add_socket(new_socket)?;

        if !addr.is_null() {
            let sockaddr_in = SockAddr::from(remote_endpoint);
            sockaddr_in.write_to(addr, addr_len)?;
        }
        Ok(new_fd.into())
    }

    pub fn sys_getsockname(
        &mut self,
        fd: FileDesc,
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_getsockname: fd={:?} addr={:?} addr_len={:?}",
            fd, addr, addr_len
        );

        let proc = self.linux_process();

        if addr.is_null() {
            return Err(LxError::EINVAL);
        }

        let socket = proc.get_socket(fd)?;
        let endpoint = socket.endpoint().ok_or(LxError::EINVAL)?;
        let sockaddr_in = SockAddr::from(endpoint);
        sockaddr_in.write_to(addr, addr_len)?;
        Ok(0)
    }

    pub fn sys_getpeername(
        &mut self,
        fd: FileDesc,
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_getpeername: fd={:?} addr={:?} addr_len={:?}",
            fd, addr, addr_len
        );

        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let proc = self.linux_process();

        if addr.is_null() {
            return Err(LxError::EINVAL);
        }

        let socket = proc.get_socket(fd)?;
        let remote_endpoint = socket.remote_endpoint().ok_or(LxError::EINVAL)?;
        let sockaddr_in = SockAddr::from(remote_endpoint);
        sockaddr_in.write_to(addr, addr_len)?;
        Ok(0)
    }
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

impl From<Endpoint> for SockAddr {
    fn from(endpoint: Endpoint) -> Self {
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
            unimplemented!("only ip");
        }
    }
}

/// Convert sockaddr to endpoint
///
/// Check len is long enough
#[allow(unsafe_code)]
fn sockaddr_to_endpoint(addr: UserInPtr<SockAddr>, len: usize) -> LxResult<Endpoint> {
    if len < size_of::<u16>() {
        return Err(LxError::EINVAL);
    }
    let addr = addr.read()?;
    if len < addr.len()? {
        return Err(LxError::EINVAL);
    }
    unsafe {
        match AddressFamily::try_from(addr.family) {
            Ok(AddressFamily::Internet) => {
                let port = u16::from_be(addr.addr_in.sin_port);
                let addr = IpAddress::from(Ipv4Address::from_bytes(
                    &u32::from_be(addr.addr_in.sin_addr).to_be_bytes()[..],
                ));
                Ok(Endpoint::Ip((addr, port).into()))
            }
            Ok(AddressFamily::Unix) => Err(LxError::EINVAL),
            Ok(AddressFamily::Packet) => Ok(Endpoint::LinkLevel(LinkLevelEndpoint::new(
                addr.addr_ll.sll_ifindex as usize,
            ))),
            Ok(AddressFamily::Netlink) => Ok(Endpoint::Netlink(NetlinkEndpoint::new(
                addr.addr_nl.nl_pid,
                addr.addr_nl.nl_groups,
            ))),
            _ => Err(LxError::EINVAL),
        }
    }
}

#[allow(unsafe_code)]
impl SockAddr {
    fn len(&self) -> LxResult<usize> {
        match AddressFamily::try_from(unsafe { self.family }) {
            Ok(AddressFamily::Internet) => Ok(size_of::<SockAddrIn>()),
            Ok(AddressFamily::Packet) => Ok(size_of::<SockAddrLl>()),
            Ok(AddressFamily::Netlink) => Ok(size_of::<SockAddrNl>()),
            Ok(AddressFamily::Unix) => Err(LxError::EINVAL),
            _ => Err(LxError::EINVAL),
        }
    }

    /// Write to user sockaddr
    /// Check mutability for user
    fn write_to(self, addr: UserOutPtr<SockAddr>, mut addr_len: UserInOutPtr<u32>) -> SysResult {
        // Ignore NULL
        if addr.is_null() {
            return Ok(0);
        }

        let max_addr_len = addr_len.read()? as usize;
        let full_len = self.len()?;

        let written_len = min(max_addr_len, full_len);
        if written_len > 0 {
            let source = unsafe {
                core::slice::from_raw_parts(&self as *const SockAddr as *const u8, written_len)
            };
            let mut addr: UserOutPtr<u8> = unsafe { core::mem::transmute(addr) };
            addr.write_array(source)?;
        }
        addr_len.write(full_len as u32)?;
        return Ok(0);
    }
}

//#[repr(C)]
//#[derive(Debug)]
//pub struct MsgHdr {
//    msg_name: *mut SockAddr,
//    msg_namelen: u32,
//    msg_iov: *mut IoVec,
//    msg_iovlen: usize,
//    msg_control: usize,
//    msg_controllen: usize,
//    msg_flags: usize,
//}

const SOCK_TYPE_MASK: u8 = 0xf;

numeric_enum! {
    #[repr(u8)]
    #[derive(Debug)]
    /// Socket types
    pub enum SocketType {
        /// Stream
        Stream = 1,
        /// Datagram
        Datagram = 2,
        /// Raw
        Raw = 3,
    }
}

//const IPPROTO_IP: usize = 0;
//const IPPROTO_ICMP: usize = 1;
const IPPROTO_TCP: usize = 6;

const SOL_SOCKET: usize = 1;
const SO_SNDBUF: usize = 7;
const SO_RCVBUF: usize = 8;
//const SO_LINGER: usize = 13;

const TCP_CONGESTION: usize = 13;

//const IP_HDRINCL: usize = 3;

#![allow(unsafe_code, missing_docs)]

use crate::error::*;
use alloc::boxed::Box;
use alloc::fmt::Debug;
use alloc::vec::Vec;
use async_trait::async_trait;
use bitflags::*;
use core::convert::TryFrom;
use core::future::Future;
use core::mem::size_of;
use core::pin::Pin;
use core::task::{Context, Poll};
use lazy_static::lazy_static;
use numeric_enum_macro::numeric_enum;
use smoltcp::socket::*;
use smoltcp::wire::*;
use spin::Mutex;

pub use smoltcp::wire;

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
    async fn connect(&mut self, endpoint: Endpoint) -> SysResult;
    fn bind(&mut self, _endpoint: Endpoint) -> SysResult {
        Err(LxError::EINVAL)
    }
    fn listen(&mut self) -> SysResult {
        Err(LxError::EINVAL)
    }
    fn shutdown(&self) -> SysResult {
        Err(LxError::EINVAL)
    }
    async fn accept(&mut self) -> LxResult<(Box<dyn Socket>, Endpoint)> {
        Err(LxError::EINVAL)
    }
    fn endpoint(&self) -> Option<Endpoint> {
        None
    }
    fn remote_endpoint(&self) -> Option<Endpoint> {
        None
    }
    fn setsockopt(&mut self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        warn!("setsockopt is unimplemented");
        Ok(0)
    }
    fn ioctl(&mut self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        warn!("ioctl is unimplemented for this socket");
        Ok(0)
    }
    fn box_clone(&self) -> Box<dyn Socket>;
}

impl Clone for Box<dyn Socket> {
    fn clone(&self) -> Self {
        self.box_clone()
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

#[derive(Debug, Clone)]
pub struct TcpSocketState {
    handle: GlobalSocketHandle,
    local_endpoint: Option<IpEndpoint>, // save local endpoint for bind()
    is_listening: bool,
}

#[derive(Debug, Clone)]
pub struct UdpSocketState {
    handle: GlobalSocketHandle,
    remote_endpoint: Option<IpEndpoint>, // remember remote endpoint for connect()
}

#[derive(Debug, Clone)]
pub struct RawSocketState {
    handle: GlobalSocketHandle,
    header_included: bool,
}

/// A wrapper for `SocketHandle`.
/// Auto increase and decrease reference count on Clone and Drop.
#[derive(Debug)]
struct GlobalSocketHandle(SocketHandle);

impl Clone for GlobalSocketHandle {
    fn clone(&self) -> Self {
        SOCKETS.lock().retain(self.0);
        Self(self.0)
    }
}

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

impl TcpSocketState {
    pub fn new() -> Self {
        let rx_buffer = TcpSocketBuffer::new(vec![0; TCP_RECVBUF]);
        let tx_buffer = TcpSocketBuffer::new(vec![0; TCP_SENDBUF]);
        let socket = TcpSocket::new(rx_buffer, tx_buffer);
        let handle = GlobalSocketHandle(SOCKETS.lock().add(socket));

        TcpSocketState {
            handle,
            local_endpoint: None,
            is_listening: false,
        }
    }
}

#[async_trait]
impl Socket for TcpSocketState {
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        loop {
            poll_ifaces();
            let mut sockets = SOCKETS.lock();
            let mut socket = sockets.get::<TcpSocket>(self.handle.0);

            if socket.may_recv() {
                if let Ok(size) = socket.recv_slice(data) {
                    if size > 0 {
                        let endpoint = socket.remote_endpoint();
                        // avoid deadlock
                        drop(socket);
                        drop(sockets);

                        poll_ifaces();
                        return (Ok(size), Endpoint::Ip(endpoint));
                    }
                }
            } else {
                return (
                    Err(LxError::ENOTCONN),
                    Endpoint::Ip(IpEndpoint::UNSPECIFIED),
                );
            }
        }
    }

    fn write(&self, data: &[u8], _sendto_endpoint: Option<Endpoint>) -> SysResult {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);

        if socket.is_open() {
            if socket.can_send() {
                match socket.send_slice(&data) {
                    Ok(size) => {
                        // avoid deadlock
                        drop(socket);
                        drop(sockets);

                        poll_ifaces();
                        Ok(size)
                    }
                    Err(_) => Err(LxError::ENOBUFS),
                }
            } else {
                Err(LxError::ENOBUFS)
            }
        } else {
            Err(LxError::ENOTCONN)
        }
    }

    fn poll(&self) -> (bool, bool, bool) {
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get::<TcpSocket>(self.handle.0);

        let (mut input, mut output, mut err) = (false, false, false);
        if self.is_listening && socket.is_active() {
            // a new connection
            input = true;
        } else if !socket.is_open() {
            err = true;
        } else {
            if socket.can_recv() {
                input = true;
            }
            if socket.can_send() {
                output = true;
            }
        }
        (input, output, err)
    }

    async fn connect(&mut self, endpoint: Endpoint) -> SysResult {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);

        if let Endpoint::Ip(ip) = endpoint {
            let temp_port = get_ephemeral_port();

            socket
                .connect(ip, temp_port)
                .map_err(|_| LxError::ENOBUFS)?;
            // avoid deadlock
            drop(socket);
            drop(sockets);

            // wait for connection result
            loop {
                poll_ifaces();

                let mut sockets = SOCKETS.lock();
                let socket = sockets.get::<TcpSocket>(self.handle.0);
                match socket.state() {
                    TcpState::SynSent => {
                        // still connecting
                        drop(socket);
                        debug!("poll for connection wait");
                        drop(sockets);
                        IFaceFuture.await;
                    }
                    TcpState::Established => {
                        return Ok(0);
                    }
                    _ => {
                        return Err(LxError::ECONNREFUSED);
                    }
                }
            }
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn bind(&mut self, endpoint: Endpoint) -> SysResult {
        if let Endpoint::Ip(mut ip) = endpoint {
            if ip.port == 0 {
                ip.port = get_ephemeral_port();
            }
            self.local_endpoint = Some(ip);
            self.is_listening = false;
            Ok(0)
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn listen(&mut self) -> SysResult {
        if self.is_listening {
            // it is ok to listen twice
            return Ok(0);
        }
        let local_endpoint = self.local_endpoint.ok_or(LxError::EINVAL)?;
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);

        info!("socket listening on {:?}", local_endpoint);
        if socket.is_listening() {
            return Ok(0);
        }
        match socket.listen(local_endpoint) {
            Ok(()) => {
                self.is_listening = true;
                Ok(0)
            }
            Err(_) => Err(LxError::EINVAL),
        }
    }

    fn shutdown(&self) -> SysResult {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<TcpSocket>(self.handle.0);
        socket.close();
        Ok(0)
    }

    async fn accept(&mut self) -> LxResult<(Box<dyn Socket>, Endpoint)> {
        let endpoint = self.local_endpoint.ok_or(LxError::EINVAL)?;
        loop {
            let mut sockets = SOCKETS.lock();
            let socket = sockets.get::<TcpSocket>(self.handle.0);

            if socket.is_active() {
                let remote_endpoint = socket.remote_endpoint();
                drop(socket);

                let new_socket = {
                    let rx_buffer = TcpSocketBuffer::new(vec![0; TCP_RECVBUF]);
                    let tx_buffer = TcpSocketBuffer::new(vec![0; TCP_SENDBUF]);
                    let mut socket = TcpSocket::new(rx_buffer, tx_buffer);
                    socket.listen(endpoint).unwrap();
                    let new_handle = GlobalSocketHandle(sockets.add(socket));
                    let old_handle = core::mem::replace(&mut self.handle, new_handle);

                    Box::new(TcpSocketState {
                        handle: old_handle,
                        local_endpoint: self.local_endpoint,
                        is_listening: false,
                    })
                };

                drop(sockets);
                poll_ifaces();
                return Ok((new_socket, Endpoint::Ip(remote_endpoint)));
            }

            drop(socket);
            drop(sockets);
            IFaceFuture.await;
        }
    }

    fn endpoint(&self) -> Option<Endpoint> {
        self.local_endpoint
            .clone()
            .map(|e| Endpoint::Ip(e))
            .or_else(|| {
                let mut sockets = SOCKETS.lock();
                let socket = sockets.get::<TcpSocket>(self.handle.0);
                let endpoint = socket.local_endpoint();
                if endpoint.port != 0 {
                    Some(Endpoint::Ip(endpoint))
                } else {
                    None
                }
            })
    }

    fn remote_endpoint(&self) -> Option<Endpoint> {
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get::<TcpSocket>(self.handle.0);
        if socket.is_open() {
            Some(Endpoint::Ip(socket.remote_endpoint()))
        } else {
            None
        }
    }

    fn box_clone(&self) -> Box<dyn Socket> {
        Box::new(self.clone())
    }
}

impl UdpSocketState {
    pub fn new() -> Self {
        let rx_buffer = UdpSocketBuffer::new(
            vec![UdpPacketMetadata::EMPTY; UDP_METADATA_BUF],
            vec![0; UDP_RECVBUF],
        );
        let tx_buffer = UdpSocketBuffer::new(
            vec![UdpPacketMetadata::EMPTY; UDP_METADATA_BUF],
            vec![0; UDP_SENDBUF],
        );
        let socket = UdpSocket::new(rx_buffer, tx_buffer);
        let handle = GlobalSocketHandle(SOCKETS.lock().add(socket));

        UdpSocketState {
            handle,
            remote_endpoint: None,
        }
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

#[async_trait]
impl Socket for UdpSocketState {
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        loop {
            let mut sockets = SOCKETS.lock();
            let mut socket = sockets.get::<UdpSocket>(self.handle.0);

            if socket.can_recv() {
                if let Ok((size, remote_endpoint)) = socket.recv_slice(data) {
                    let endpoint = remote_endpoint;
                    // avoid deadlock
                    drop(socket);
                    drop(sockets);

                    poll_ifaces();
                    return (Ok(size), Endpoint::Ip(endpoint));
                }
            } else {
                return (
                    Err(LxError::ENOTCONN),
                    Endpoint::Ip(IpEndpoint::UNSPECIFIED),
                );
            }

            drop(socket);
            drop(sockets);
            IFaceFuture.await;
        }
    }

    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult {
        let remote_endpoint = {
            if let Some(Endpoint::Ip(ref endpoint)) = sendto_endpoint {
                endpoint
            } else if let Some(ref endpoint) = self.remote_endpoint {
                endpoint
            } else {
                return Err(LxError::ENOTCONN);
            }
        };
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<UdpSocket>(self.handle.0);

        if socket.endpoint().port == 0 {
            let temp_port = get_ephemeral_port();
            socket
                .bind(IpEndpoint::new(IpAddress::Unspecified, temp_port))
                .unwrap();
        }

        if socket.can_send() {
            match socket.send_slice(&data, *remote_endpoint) {
                Ok(()) => {
                    // avoid deadlock
                    drop(socket);
                    drop(sockets);

                    poll_ifaces();
                    Ok(data.len())
                }
                Err(_) => Err(LxError::ENOBUFS),
            }
        } else {
            Err(LxError::ENOBUFS)
        }
    }

    fn poll(&self) -> (bool, bool, bool) {
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get::<UdpSocket>(self.handle.0);

        let (mut input, mut output, err) = (false, false, false);
        if socket.can_recv() {
            input = true;
        }
        if socket.can_send() {
            output = true;
        }
        (input, output, err)
    }

    async fn connect(&mut self, endpoint: Endpoint) -> SysResult {
        if let Endpoint::Ip(ip) = endpoint {
            self.remote_endpoint = Some(ip);
            Ok(0)
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn bind(&mut self, endpoint: Endpoint) -> SysResult {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<UdpSocket>(self.handle.0);
        if let Endpoint::Ip(ip) = endpoint {
            match socket.bind(ip) {
                Ok(()) => Ok(0),
                Err(_) => Err(LxError::EINVAL),
            }
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn ioctl(&mut self, request: usize, arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        match request {
            // SIOCGARP
            0x8954 => {
                // FIXME: check addr
                let req = unsafe { &mut *(arg1 as *mut ArpReq) };
                if let AddressFamily::Internet = AddressFamily::try_from(req.arp_pa.family).unwrap()
                {
                    unimplemented!();

                //                    let ifname = req.iface_name();
                //                    let addr = &req.arp_pa as *const SockAddrPlaceholder as *const SockAddr;
                //                    let addr = unsafe {
                //                        IpAddress::from(Ipv4Address::from_bytes(
                //                            &u32::from_be((*addr).addr_in.sin_addr).to_be_bytes()[..],
                //                        ))
                //                    };
                //                    for iface in NET_DRIVERS.read().iter() {
                //                        if iface.get_ifname() == ifname {
                //                            debug!("get arp matched ifname {}", ifname);
                //                            return match iface.get_arp(addr) {
                //                                Some(mac) => {
                //                                    // TODO: update flags
                //                                    req.arp_ha.data[0..6].copy_from_slice(mac.as_bytes());
                //                                    Ok(0)
                //                                }
                //                                None => Err(LxError::ENOENT),
                //                            };
                //                        }
                //                    }
                //                    Err(LxError::ENOENT)
                } else {
                    Err(LxError::EINVAL)
                }
            }
            _ => Ok(0),
        }
    }

    fn endpoint(&self) -> Option<Endpoint> {
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get::<UdpSocket>(self.handle.0);
        let endpoint = socket.endpoint();
        if endpoint.port != 0 {
            Some(Endpoint::Ip(endpoint))
        } else {
            None
        }
    }

    fn remote_endpoint(&self) -> Option<Endpoint> {
        self.remote_endpoint.clone().map(|e| Endpoint::Ip(e))
    }

    fn box_clone(&self) -> Box<dyn Socket> {
        Box::new(self.clone())
    }
}

impl RawSocketState {
    pub fn new(protocol: u8) -> Self {
        let rx_buffer = RawSocketBuffer::new(
            vec![RawPacketMetadata::EMPTY; RAW_METADATA_BUF],
            vec![0; RAW_RECVBUF],
        );
        let tx_buffer = RawSocketBuffer::new(
            vec![RawPacketMetadata::EMPTY; RAW_METADATA_BUF],
            vec![0; RAW_SENDBUF],
        );
        let socket = RawSocket::new(
            IpVersion::Ipv4,
            IpProtocol::from(protocol),
            rx_buffer,
            tx_buffer,
        );
        let handle = GlobalSocketHandle(SOCKETS.lock().add(socket));

        RawSocketState {
            handle,
            header_included: false,
        }
    }
}

#[async_trait]
impl Socket for RawSocketState {
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        loop {
            let mut sockets = SOCKETS.lock();
            let mut socket = sockets.get::<RawSocket>(self.handle.0);

            if let Ok(size) = socket.recv_slice(data) {
                let packet = Ipv4Packet::new_unchecked(data);

                return (
                    Ok(size),
                    Endpoint::Ip(IpEndpoint {
                        addr: IpAddress::Ipv4(packet.src_addr()),
                        port: 0,
                    }),
                );
            }

            drop(socket);
            drop(sockets);
            IFaceFuture.await;
        }
    }

    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult {
        if self.header_included {
            let mut sockets = SOCKETS.lock();
            let mut socket = sockets.get::<RawSocket>(self.handle.0);

            match socket.send_slice(&data) {
                Ok(()) => Ok(data.len()),
                Err(_) => Err(LxError::ENOBUFS),
            }
        } else {
            if let Some(Endpoint::Ip(_endpoint)) = sendto_endpoint {
                unimplemented!();
            // temporary solution
            //                let iface = &*(NET_DRIVERS.read()[0]);
            //                let v4_src = iface.ipv4_address().unwrap();
            //                let mut sockets = SOCKETS.lock();
            //                let mut socket = sockets.get::<RawSocket>(self.handle.0);
            //
            //                if let IpAddress::Ipv4(v4_dst) = endpoint.addr {
            //                    let len = data.len();
            //                    // using 20-byte IPv4 header
            //                    let mut buffer = vec![0u8; len + 20];
            //                    let mut packet = Ipv4Packet::new_unchecked(&mut buffer);
            //                    packet.set_version(4);
            //                    packet.set_header_len(20);
            //                    packet.set_total_len((20 + len) as u16);
            //                    packet.set_protocol(socket.ip_protocol().into());
            //                    packet.set_src_addr(v4_src);
            //                    packet.set_dst_addr(v4_dst);
            //                    let payload = packet.payload_mut();
            //                    payload.copy_from_slice(data);
            //                    packet.fill_checksum();
            //
            //                    socket.send_slice(&buffer).unwrap();
            //
            //                    // avoid deadlock
            //                    drop(socket);
            //                    drop(sockets);
            //                    iface.poll();
            //
            //                    Ok(len)
            //                } else {
            //                    unimplemented!("ip type")
            //                }
            } else {
                Err(LxError::ENOTCONN)
            }
        }
    }

    fn poll(&self) -> (bool, bool, bool) {
        unimplemented!()
    }

    async fn connect(&mut self, _endpoint: Endpoint) -> SysResult {
        unimplemented!()
    }

    fn box_clone(&self) -> Box<dyn Socket> {
        Box::new(self.clone())
    }

    fn setsockopt(&mut self, level: usize, opt: usize, data: &[u8]) -> SysResult {
        match (level, opt) {
            (IPPROTO_IP, IP_HDRINCL) => {
                if let Some(arg) = data.first() {
                    self.header_included = *arg > 0;
                    debug!("hdrincl set to {}", self.header_included);
                }
            }
            _ => {}
        }
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

pub const TCP_SENDBUF: usize = 512 * 1024; // 512K
pub const TCP_RECVBUF: usize = 512 * 1024; // 512K

const UDP_METADATA_BUF: usize = 1024;
const UDP_SENDBUF: usize = 64 * 1024; // 64K
const UDP_RECVBUF: usize = 64 * 1024; // 64K

const RAW_METADATA_BUF: usize = 1024;
const RAW_SENDBUF: usize = 64 * 1024; // 64K
const RAW_RECVBUF: usize = 64 * 1024; // 64K

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

struct IFaceFuture;

impl Future for IFaceFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        unimplemented!()
    }
}

const IPPROTO_IP: usize = 0;
const IP_HDRINCL: usize = 3;

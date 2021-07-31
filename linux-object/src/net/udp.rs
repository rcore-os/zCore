// udpsocket
#![allow(dead_code)]
// crate
use crate::error::LxError;
use crate::error::LxResult;
use crate::fs::FileLike;
use crate::fs::FileLikeType;
use crate::net::from_cstr;
use crate::net::get_ephemeral_port;
use crate::net::get_net_driver;
use crate::net::poll_ifaces;
use crate::net::AddressFamily;
use crate::net::ArpReq;
use crate::net::Endpoint;
use crate::net::GlobalSocketHandle;
use crate::net::IpAddress;
use crate::net::IpEndpoint;
use crate::net::Ipv4Address;
use crate::net::SockAddr;
use crate::net::SockAddrPlaceholder;
use crate::net::SOCKETS;
use crate::net::UDP_METADATA_BUF;
use crate::net::UDP_RECVBUF;
use crate::net::UDP_SENDBUF;

// alloc
use alloc::boxed::Box;
use alloc::vec;

// smoltcp

use smoltcp::socket::UdpPacketMetadata;
use smoltcp::socket::UdpSocket;
use smoltcp::socket::UdpSocketBuffer;

// async
use async_trait::async_trait;

// third part
use rcore_fs::vfs::PollStatus;
use zircon_object::impl_kobject;
use zircon_object::object::*;

/// missing documentation
pub struct UdpSocketState {
    /// missing documentation
    base: KObjectBase,
    /// missing documentation
    handle: GlobalSocketHandle,
    /// missing documentation
    remote_endpoint: Option<IpEndpoint>, // remember remote endpoint for connect()
}

impl Default for UdpSocketState {
    fn default() -> Self {
        UdpSocketState::new()
    }
}

impl UdpSocketState {
    /// missing documentation
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
            base: KObjectBase::new(),
            handle,
            remote_endpoint: None,
        }
    }

    fn default() -> Self {
        Self::new()
    }

    /// missing documentation
    pub fn udp_read(&self, data: &mut [u8]) -> (LxResult<usize>, Endpoint) {
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
        }
    }

    /// missing documentation
    pub fn udp_write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> LxResult<usize> {
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
            match socket.send_slice(data, *remote_endpoint) {
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

    /// missing documentation
    pub fn poll(&self) -> (bool, bool, bool) {
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

    fn connect(&mut self, endpoint: Endpoint) -> LxResult<usize> {
        #[allow(irrefutable_let_patterns)]
        if let Endpoint::Ip(ip) = endpoint {
            self.remote_endpoint = Some(ip);
            Ok(0)
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn bind(&mut self, endpoint: Endpoint) -> LxResult<usize> {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<UdpSocket>(self.handle.0);
        #[allow(irrefutable_let_patterns)]
        if let Endpoint::Ip(ip) = endpoint {
            match socket.bind(ip) {
                Ok(()) => Ok(0),
                Err(_) => Err(LxError::EINVAL),
            }
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn udp_ioctl(
        &self,
        request: usize,
        arg1: usize,
        _arg2: usize,
        _arg3: usize,
    ) -> LxResult<usize> {
        match request {
            // SIOCGARP
            0x8954 => {
                // TODO: check addr
                #[allow(unsafe_code)]
                let req = unsafe { &mut *(arg1 as *mut ArpReq) };
                if let AddressFamily::Internet = AddressFamily::from(req.arp_pa.family) {
                    let name = req.arp_dev.as_ptr();
                    #[allow(unsafe_code)]
                    let ifname = unsafe { from_cstr(name) };
                    let addr = &req.arp_pa as *const SockAddrPlaceholder as *const SockAddr;
                    #[allow(unsafe_code)]
                    let addr = unsafe {
                        IpAddress::from(Ipv4Address::from_bytes(
                            &u32::from_be((*addr).addr_in.sin_addr).to_be_bytes()[..],
                        ))
                    };
                    for iface in get_net_driver().iter() {
                        if iface.get_ifname() == ifname {
                            debug!("get arp matched ifname {}", ifname);
                            return match iface.get_arp(addr) {
                                Some(mac) => {
                                    // TODO: update flags
                                    req.arp_ha.data[0..6].copy_from_slice(mac.as_bytes());
                                    Ok(0)
                                }
                                None => Err(LxError::ENOENT),
                            };
                        }
                    }
                    Err(LxError::ENOENT)
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
        self.remote_endpoint.map(Endpoint::Ip)
    }
}
impl_kobject!(UdpSocketState);

#[async_trait]
impl FileLike for UdpSocketState {
    /// read to buffer
    async fn read(&self, _buf: &mut [u8]) -> LxResult<usize> {
        unimplemented!()
    }
    /// write from buffer
    fn write(&self, _buf: &[u8]) -> LxResult<usize> {
        unimplemented!()
    }
    /// read to buffer at given offset
    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
        unimplemented!()
    }
    /// write from buffer at given offset
    fn write_at(&self, _offset: u64, _buf: &[u8]) -> LxResult<usize> {
        unimplemented!()
    }
    /// wait for some event on a file descriptor
    fn poll(&self) -> LxResult<PollStatus> {
        unimplemented!()
    }
    /// wait for some event on a file descriptor use async
    async fn async_poll(&self) -> LxResult<PollStatus> {
        unimplemented!()
    }
    /// manipulates the underlying device parameters of special files
    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize> {
        self.udp_ioctl(request, arg1, arg2, arg3)
        // unimplemented!()
    }
    /// manipulate file descriptor
    fn fcntl(&self, _cmd: usize, _arg: usize) -> LxResult<usize> {
        Ok(0)
        // unimplemented!()
    }
    /// file type
    fn file_type(&self) -> LxResult<FileLikeType> {
        Ok(FileLikeType::UdpSocket)
    }
}

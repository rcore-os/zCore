// udpsocket

use crate::error::{LxError, LxResult};
use crate::fs::{FileLike, OpenFlags, PollStatus};
use crate::net::*;
use alloc::{boxed::Box, sync::Arc, vec};
use async_trait::async_trait;
use lock::{Mutex, RwLock};
use smoltcp::socket::{UdpPacketMetadata, UdpSocket, UdpSocketBuffer};

// third part
#[allow(unused_imports)]
use zircon_object::impl_kobject;
#[allow(unused_imports)]
use zircon_object::object::*;

/// UDP socket structure
pub struct UdpSocketState {
    /// Kernel object base
    base: KObjectBase,
    /// A wrapper for `SocketHandle`
    handle: GlobalSocketHandle,
    /// remember remote endpoint for connect fn
    remote_endpoint: Option<IpEndpoint>,
    /// flags on the socket
    flags: RwLock<SocketFlags>,
}

impl Default for UdpSocketState {
    fn default() -> Self {
        UdpSocketState::new()
    }
}

impl UdpSocketState {
    /// missing documentation
    pub fn new() -> Self {
        info!("udp new");
        let rx_buffer = UdpSocketBuffer::new(
            vec![UdpPacketMetadata::EMPTY; UDP_METADATA_BUF],
            vec![0; UDP_RECVBUF],
        );
        let tx_buffer = UdpSocketBuffer::new(
            vec![UdpPacketMetadata::EMPTY; UDP_METADATA_BUF],
            vec![0; UDP_SENDBUF],
        );
        let socket = UdpSocket::new(rx_buffer, tx_buffer);
        let handle = GlobalSocketHandle(get_sockets().lock().add(socket));

        UdpSocketState {
            base: KObjectBase::new(),
            handle,
            remote_endpoint: None,
            flags: RwLock::new(SocketFlags::empty()),
        }
    }
}

/// missing in implementation
#[async_trait]
impl Socket for UdpSocketState {
    /// read to buffer
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        info!("udp read");
        loop {
            info!("udp read loop");
            poll_ifaces();
            let net_sockets = get_sockets();
            let mut sockets = net_sockets.lock();
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
        }
    }
    /// write from buffer
    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult {
        info!("udp write");
        let remote_endpoint = {
            if let Some(Endpoint::Ip(ref endpoint)) = sendto_endpoint {
                endpoint
            } else if let Some(ref endpoint) = self.remote_endpoint {
                endpoint
            } else {
                return Err(LxError::ENOTCONN);
            }
        };

        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
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
    /// connect
    async fn connect(&mut self, endpoint: Endpoint) -> SysResult {
        if let Endpoint::Ip(ip) = endpoint {
            self.remote_endpoint = Some(ip);
            Ok(0)
        } else {
            Err(LxError::EINVAL)
        }
    }
    /// wait for some event on a file descriptor
    fn poll(&self) -> (bool, bool, bool) {
        let sockets = get_sockets();
        let mut set = sockets.lock();
        let socket = set.get::<UdpSocket>(self.handle.0);

        let (mut input, mut output, mut err) = (false, false, false);
        if !socket.is_open() {
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

    fn bind(&mut self, endpoint: Endpoint) -> SysResult {
        info!("udp bind");
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let mut socket = sockets.get::<UdpSocket>(self.handle.0);
        #[allow(irrefutable_let_patterns)]
        if let Endpoint::Ip(mut ip) = endpoint {
            if ip.port == 0 {
                ip.port = get_ephemeral_port();
            }
            match socket.bind(ip) {
                Ok(()) => Ok(0),
                Err(_) => Err(LxError::EINVAL),
            }
        } else {
            Err(LxError::EINVAL)
        }
    }
    fn listen(&mut self) -> SysResult {
        Err(LxError::EINVAL)
    }
    fn shutdown(&self) -> SysResult {
        Err(LxError::EINVAL)
    }
    async fn accept(&mut self) -> LxResult<(Arc<Mutex<dyn Socket>>, Endpoint)> {
        Err(LxError::EINVAL)
    }
    fn endpoint(&self) -> Option<Endpoint> {
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
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
    fn setsockopt(&mut self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        warn!("setsockopt is unimplemented");
        Ok(0)
    }

    /// manipulate file descriptor
    fn ioctl(&self, request: usize, arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        warn!("ioctl is unimplemented for this socket");
        info!("udp ioctrl");
        match request {
            // SIOCGARP
            0x8954 => {
                // TODO: check addr
                #[allow(unsafe_code)]
                let req = unsafe { &mut *(arg1 as *mut ArpReq) };
                if let AddressFamily::Internet = AddressFamily::from(req.arp_pa.family) {
                    let name = req.arp_dev.as_ptr();
                    #[allow(unsafe_code)]
                    let _ifname = unsafe { from_cstr(name) };
                    let addr = &req.arp_pa as *const SockAddrPlaceholder as *const SockAddr;
                    #[allow(unsafe_code)]
                    let _addr = unsafe {
                        IpAddress::from(Ipv4Address::from_bytes(
                            &u32::from_be((*addr).addr_in.sin_addr).to_be_bytes()[..],
                        ))
                    };
                    // for iface in get_net_device().iter() {
                    //     if iface.get_ifname() == ifname {
                    //         debug!("get arp matched ifname {}", ifname);
                    //         return match iface.get_arp(addr) {
                    //             Some(mac) => {
                    //                 // TODO: update flags
                    //                 req.arp_ha.data[0..6].copy_from_slice(mac.as_bytes());
                    //                 Ok(0)
                    //             }
                    //             None => Err(LxError::ENOENT),
                    //         };
                    //     }
                    // }
                    Err(LxError::ENOENT)
                } else {
                    Err(LxError::EINVAL)
                }
            }
            _ => Ok(0),
        }
    }

    fn fcntl(&self, _cmd: usize, _arg: usize) -> SysResult {
        warn!("fnctl is unimplemented for this socket");
        Ok(0)
    }
}

impl_kobject!(UdpSocketState);

#[async_trait]
impl FileLike for UdpSocketState {
    fn flags(&self) -> OpenFlags {
        let f = self.flags.read();
        let mut open_flags = OpenFlags::empty();
        open_flags.set(OpenFlags::NON_BLOCK, f.contains(SocketFlags::SOCK_NONBLOCK));
        open_flags.set(OpenFlags::CLOEXEC, f.contains(SocketFlags::SOCK_CLOEXEC));
        open_flags
    }

    fn set_flags(&self, f: OpenFlags) -> LxResult {
        let flags = &mut self.flags.write();
        flags.set(SocketFlags::SOCK_NONBLOCK, f.contains(OpenFlags::NON_BLOCK));
        flags.set(SocketFlags::SOCK_CLOEXEC, f.contains(OpenFlags::CLOEXEC));
        Ok(())
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        Socket::read(self, buf).await.0
    }

    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
        unimplemented!()
    }

    fn write(&self, buf: &[u8]) -> LxResult<usize> {
        Socket::write(self, buf, None)
    }

    fn poll(&self) -> LxResult<PollStatus> {
        let (read, write, error) = Socket::poll(self);
        Ok(PollStatus { read, write, error })
    }

    async fn async_poll(&self) -> LxResult<PollStatus> {
        unimplemented!()
    }

    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize> {
        Socket::ioctl(self, request, arg1, arg2, arg3)
    }
}

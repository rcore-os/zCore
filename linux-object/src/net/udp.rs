use super::*;
use smoltcp::socket::{UdpSocket, UdpSocketBuffer, UdpPacketMetadata};

#[derive(Debug)]
pub struct UdpSocketState {
    handle: GlobalSocketHandle,
    remote_endpoint: Option<IpEndpoint>, // remember remote endpoint for connect()
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

impl Drop for UdpSocketState {
    fn drop(&self) {
        let mut sockets = self.sockets.lock();
        sockets.release(self.handle);
        sockets.prune();

        // send FIN immediately when applicable
        drop(sockets);
        poll_ifaces();
    }
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

    async fn connect(&self, endpoint: Endpoint) -> SysResult {
        if let Endpoint::Ip(ip) = endpoint {
            self.remote_endpoint = Some(ip);
            Ok(0)
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn bind(&self, endpoint: Endpoint) -> SysResult {
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

    fn ioctl(&self, request: usize, arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        match request {
            // SIOCGARP
            0x8954 => {
                // FIXME: check addr
                let req = unsafe { &mut *(arg1 as *mut ArpReq) };
                if let AddressFamily::Internet = AddressFamily::from(req.arp_pa.family) {
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
}

const UDP_METADATA_BUF: usize = 1024;
const UDP_SENDBUF: usize = 64 * 1024; // 64K
const UDP_RECVBUF: usize = 64 * 1024; // 64K

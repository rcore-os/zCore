use crate::{error::LxError, net::*};
use alloc::boxed::Box;
use async_trait::async_trait;
use smoltcp::{
    socket::{RawPacketMetadata, RawSocket, RawSocketBuffer},
    wire::{IpProtocol, IpVersion, Ipv4Address, Ipv4Packet},
};

/// missing documentation
#[derive(Debug, Clone)]
pub struct RawSocketState {
    handle: GlobalSocketHandle,
    header_included: bool,
}

impl RawSocketState {
    /// missing documentation
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
        let handle = GlobalSocketHandle(get_sockets().lock().add(socket));

        RawSocketState {
            handle,
            header_included: false,
        }
    }
}

/// missing in implementation
#[async_trait]
impl Socket for RawSocketState {
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        info!("raw read");
        loop {
            info!("raw read loop");
            poll_ifaces();
            let net_sockets = get_sockets();
            let mut sockets = net_sockets.lock();
            let mut socket = sockets.get::<RawSocket>(self.handle.0);
            if socket.can_recv() {
                if let Ok(size) = socket.recv_slice(data) {
                    let packet = Ipv4Packet::new_unchecked(data);
                    // avoid deadlock
                    drop(socket);
                    drop(sockets);
                    poll_ifaces();
                    return (
                        Ok(size),
                        Endpoint::Ip(IpEndpoint {
                            addr: IpAddress::Ipv4(packet.src_addr()),
                            port: 0,
                        }),
                    );
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

    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult {
        info!("raw write");
        let net_sockets = get_sockets();
        let mut sockets = net_sockets.lock();
        let mut socket = sockets.get::<RawSocket>(self.handle.0);
        if self.header_included {
            match socket.send_slice(data) {
                Ok(()) => Ok(data.len()),
                Err(_) => Err(LxError::ENOBUFS),
            }
        } else if let Some(Endpoint::Ip(endpoint)) = sendto_endpoint {
            // todo: this is a temporary solution
            let v4_src = Ipv4Address::new(192, 168, 0, 123);

            if let IpAddress::Ipv4(v4_dst) = endpoint.addr {
                let len = data.len();
                // using 20-byte IPv4 header
                let mut buffer = vec![0u8; len + 20];
                let mut packet = Ipv4Packet::new_unchecked(&mut buffer);
                packet.set_version(4);
                packet.set_header_len(20);
                packet.set_total_len((20 + len) as u16);
                packet.set_protocol(socket.ip_protocol());
                packet.set_src_addr(v4_src);
                packet.set_dst_addr(v4_dst);
                let payload = packet.payload_mut();
                payload.copy_from_slice(data);
                packet.fill_checksum();

                socket.send_slice(&buffer).unwrap();

                // avoid deadlock
                drop(socket);
                drop(sockets);
                Ok(len)
            } else {
                unimplemented!("ip type")
            }
        } else {
            Err(LxError::ENOTCONN)
        }
    }

    async fn connect(&self, _endpoint: Endpoint) -> SysResult {
        unimplemented!()
    }

    fn setsockopt(&self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        // match (level, opt) {
        //     (IPPROTO_IP, IP_HDRINCL) => {
        //         if let Some(arg) = data.first() {
        //             self.header_included = *arg > 0;
        //             debug!("hdrincl set to {}", self.header_included);
        //         }
        //     }
        //     _ => {}
        // }
        Ok(0)
    }
    fn get_buffer_capacity(&self) -> Option<(usize, usize)> {
        let sockets = get_sockets();
        let mut s = sockets.lock();
        let socket = s.get::<RawSocket>(self.handle.0);
        let (recv_ca, send_ca) = (
            socket.payload_recv_capacity(),
            socket.payload_send_capacity(),
        );
        Some((recv_ca, send_ca))
    }
    fn socket_type(&self) -> Option<SocketType> {
        Some(SocketType::SOCK_RAW)
    }
}

use super::*;
use smoltcp::socket::{RawPacketMetadata, RawSocket, RawSocketBuffer};

#[derive(Debug)]
pub struct RawSocketState {
    handle: GlobalSocketHandle,
    header_included: bool,
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

    async fn connect(&self, _endpoint: Endpoint) -> SysResult {
        unimplemented!()
    }

    fn setsockopt(&self, level: usize, opt: usize, data: &[u8]) -> SysResult {
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

const RAW_METADATA_BUF: usize = 1024;
const RAW_SENDBUF: usize = 64 * 1024; // 64K
const RAW_RECVBUF: usize = 64 * 1024; // 64K

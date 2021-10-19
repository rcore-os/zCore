// rawsocket
#![allow(dead_code)]
// crate
use helper::error::LxError;
use helper::error::LxResult;
use crate::net::get_net_driver;
use crate::net::get_net_sockets;
use crate::net::Endpoint;
use crate::net::GlobalSocketHandle;
use crate::net::IpAddress;
use crate::net::IpEndpoint;
use crate::net::Socket;
use crate::net::SysResult;
use crate::net::IPPROTO_IP;
use crate::net::IP_HDRINCL;
use crate::net::RAW_METADATA_BUF;
use crate::net::RAW_RECVBUF;
use crate::net::RAW_SENDBUF;
use alloc::sync::Arc;
use spin::Mutex;
// alloc
use alloc::boxed::Box;
use alloc::vec;

// smoltcp

use smoltcp::socket::RawPacketMetadata;
use smoltcp::socket::RawSocket;
use smoltcp::socket::RawSocketBuffer;

use smoltcp::wire::IpProtocol;
use smoltcp::wire::IpVersion;
use smoltcp::wire::Ipv4Packet;

// async
use async_trait::async_trait;

// third part
use zircon_object::impl_kobject;
use zircon_object::object::*;

/// missing documentation
pub struct RawSocketState {
    /// missing documentation
    base: KObjectBase,
    /// missing documentation
    handle: GlobalSocketHandle,
    /// missing documentation
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
        let handle = GlobalSocketHandle(get_net_sockets().lock().add(socket));

        RawSocketState {
            base: KObjectBase::new(),
            handle,
            header_included: false,
        }
    }

    /// missing documentation
    pub async fn read(&self, data: &mut [u8]) -> (LxResult<usize>, Endpoint) {
        loop {
            let net_sockets = get_net_sockets();
            let mut sockets = net_sockets.lock();
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
        }
    }

    /// missing documentation
    pub fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> LxResult<usize> {
        if self.header_included {
            let net_sockets = get_net_sockets();
            let mut sockets = net_sockets.lock();
            let mut socket = sockets.get::<RawSocket>(self.handle.0);

            match socket.send_slice(data) {
                Ok(()) => Ok(data.len()),
                Err(_) => Err(LxError::ENOBUFS),
            }
        } else if let Some(Endpoint::Ip(endpoint)) = sendto_endpoint {
            // temporary solution
            let iface = &*(get_net_driver()[0]);
            let v4_src = iface.ipv4_address().unwrap();
            let net_sockets = get_net_sockets();
            let mut sockets = net_sockets.lock();
            let mut socket = sockets.get::<RawSocket>(self.handle.0);

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
                if let Ok(_) = iface.poll(&(*get_net_sockets())) {};

                Ok(len)
            } else {
                unimplemented!("ip type")
            }
        } else {
            Err(LxError::ENOTCONN)
        }
    }

    /// missing documentation
    pub fn setsockopt(&mut self, level: usize, opt: usize, data: &[u8]) -> SysResult {
        if let (IPPROTO_IP, IP_HDRINCL) = (level, opt) {
            if let Some(arg) = data.first() {
                self.header_included = *arg > 0;
                debug!("hdrincl set to {}", self.header_included);
            }
        }
        Ok(0)
    }
}
impl_kobject!(RawSocketState);

#[async_trait]
impl Socket for RawSocketState {
    /// read to buffer
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        self.read(data).await
    }
    /// write from buffer
    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult {
        self.write(data, sendto_endpoint)
    }
    /// connect
    async fn connect(&self, _endpoint: Endpoint) -> SysResult {
        unimplemented!()
        // self.connect(_endpoint).await
    }
    /// wait for some event on a file descriptor
    fn poll(&self) -> (bool, bool, bool) {
        unimplemented!()
        // self.poll()
    }

    fn bind(&mut self, _endpoint: Endpoint) -> SysResult {
        unimplemented!()
        // self.bind(endpoint)
    }
    fn listen(&mut self) -> SysResult {
        unimplemented!()
        // self.listen()
    }
    fn shutdown(&self) -> SysResult {
        unimplemented!()
        // self.shutdown()
    }
    async fn accept(&mut self) -> LxResult<(Arc<Mutex<dyn Socket>>, Endpoint)> {
        unimplemented!()
        // self.accept().await
    }
    fn endpoint(&self) -> Option<Endpoint> {
        unimplemented!()
        // self.endpoint()
    }
    fn remote_endpoint(&self) -> Option<Endpoint> {
        unimplemented!()
        // self.remote_endpoint()
    }
    fn setsockopt(&mut self, level: usize, opt: usize, data: &[u8]) -> SysResult {
        self.setsockopt(level, opt, data)
    }

    /// manipulate file descriptor
    fn ioctl(&self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        Ok(0)
    }

    fn fcntl(&self, _cmd: usize, _arg: usize) -> SysResult {
        Ok(0)
    }
}

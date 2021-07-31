// rawsocket 
#![allow(dead_code)]
// crate
use crate::fs::FileLikeType;
use crate::net::IPPROTO_IP;
use crate::net::IP_HDRINCL;
use crate::net::get_net_driver;
use crate::net::IpAddress;
use crate::net::IpEndpoint;
use crate::net::Endpoint;
use crate::net::RAW_SENDBUF;
use crate::net::RAW_RECVBUF;
use crate::net::RAW_METADATA_BUF;
use crate::net::SOCKETS;

use crate::net::GlobalSocketHandle;

use crate::error::LxResult;
use crate::error::LxError;
use crate::fs::FileLike;

// alloc
use alloc::vec;
use alloc::boxed::Box;

// smoltcp

use smoltcp::socket::RawSocketBuffer;
use smoltcp::socket::RawPacketMetadata;
use smoltcp::socket::RawSocket;

use smoltcp::wire::IpProtocol;
use smoltcp::wire::IpVersion;
use smoltcp::wire::Ipv4Packet;


// async
use async_trait::async_trait;

// third part 
use zircon_object::impl_kobject;
use rcore_fs::vfs::PollStatus;
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
        let handle = GlobalSocketHandle(SOCKETS.lock().add(socket));

        RawSocketState {
            base: KObjectBase::new(),
            handle,
            header_included: false,
        }
    }

    /// missing documentation
    pub fn raw_read(&self, data: &mut [u8]) -> (LxResult<usize>, Endpoint) {
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
            // SOCKET_ACTIVITY.wait(sockets);
        }
    }

    /// missing documentation
    pub fn raw_write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> LxResult<usize> {
        if self.header_included {
            let mut sockets = SOCKETS.lock();
            let mut socket = sockets.get::<RawSocket>(self.handle.0);

            match socket.send_slice(&data) {
                Ok(()) => Ok(data.len()),
                Err(_) => Err(LxError::ENOBUFS),
            }
        } else {
            if let Some(Endpoint::Ip(endpoint)) = sendto_endpoint {
                // temporary solution
                let iface = &*(get_net_driver()[0]);
                let v4_src = iface.ipv4_address().unwrap();
                let mut sockets = SOCKETS.lock();
                let mut socket = sockets.get::<RawSocket>(self.handle.0);

                if let IpAddress::Ipv4(v4_dst) = endpoint.addr {
                    let len = data.len();
                    // using 20-byte IPv4 header
                    let mut buffer = vec![0u8; len + 20];
                    let mut packet = Ipv4Packet::new_unchecked(&mut buffer);
                    packet.set_version(4);
                    packet.set_header_len(20);
                    packet.set_total_len((20 + len) as u16);
                    packet.set_protocol(socket.ip_protocol().into());
                    packet.set_src_addr(v4_src);
                    packet.set_dst_addr(v4_dst);
                    let payload = packet.payload_mut();
                    payload.copy_from_slice(data);
                    packet.fill_checksum();

                    socket.send_slice(&buffer).unwrap();

                    // avoid deadlock
                    drop(socket);
                    drop(sockets);
                    iface.poll(&(*SOCKETS));

                    Ok(len)
                } else {
                    unimplemented!("ip type")
                }
            } else {
                Err(LxError::ENOTCONN)
            }
        }
    }

    /// missing documentation
    pub fn raw_setsockopt(&mut self, level: usize, opt: usize, data: &[u8]) -> LxResult<usize> {
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
impl_kobject!(RawSocketState);



#[async_trait]
impl FileLike for RawSocketState {
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
    fn ioctl(&self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> LxResult<usize> {
        unimplemented!()
    }
    /// manipulate file descriptor
    fn fcntl(&self, _cmd: usize, _arg: usize) -> LxResult<usize> {
        unimplemented!()
    }
    /// file type
    fn file_type(&self) -> LxResult<FileLikeType> {
        Ok(FileLikeType::RawSocket)
    }
}
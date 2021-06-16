// icmpsocket 
#![allow(dead_code)]
// crate
use crate::net::IpAddress;
use smoltcp::socket::IcmpEndpoint;
use crate::net::Endpoint;
use crate::net::SOCKETS;
use crate::net::GlobalSocketHandle;
use crate::net::ICMP_SENDBUF;
use crate::net::ICMP_RECVBUF;
use crate::net::ICMP_METADATA_BUF;
// use crate::net::poll_ifaces;
use crate::error::LxResult;
// use crate::error::LxError;
use crate::fs::FileLike;

// alloc
use alloc::vec;
use alloc::boxed::Box;

// smoltcp

use smoltcp::socket::IcmpSocket;
use smoltcp::socket::IcmpSocketBuffer;
use smoltcp::socket::IcmpPacketMetadata;

// async
use async_trait::async_trait;

// third part 
use zircon_object::impl_kobject;
use rcore_fs::vfs::PollStatus;
use zircon_object::object::*;

/// missing documentation
pub struct IcmpSocketState {
    /// missing documentation
    base: KObjectBase,
    /// missing documentation
    handle: GlobalSocketHandle,
}


impl IcmpSocketState {
    /// missing documentation
    pub fn new() -> Self {
        let rx_buffer = IcmpSocketBuffer::new(
            vec![IcmpPacketMetadata::EMPTY; ICMP_METADATA_BUF],
            vec![0; ICMP_RECVBUF],
        );
        let tx_buffer = IcmpSocketBuffer::new(
            vec![IcmpPacketMetadata::EMPTY; ICMP_METADATA_BUF],
            vec![0; ICMP_SENDBUF],
        );
        let socket = IcmpSocket::new(rx_buffer, tx_buffer);
        let handle = GlobalSocketHandle(SOCKETS.lock().add(socket));

        IcmpSocketState {
            base: KObjectBase::new(),
            handle,
        }
    }

    /// missing documentation
    pub fn icmp_read(&self,data:&mut [u8]) -> LxResult<usize> {
        // loop {
        //     let mut sockets = SOCKETS.lock();
        //     let mut socket = sockets.get::<IcmpSocket>(self.handle.0);

        //     if socket.can_recv() {
        //         if let Ok((size, _)) = socket.recv_slice(data) {
        //             // let endpoint = remote_endpoint;
        //             // avoid deadlock
        //             drop(socket);
        //             drop(sockets);

        //             poll_ifaces();
        //             return Ok(size);
        //         }
        //     } else {
        //         return Err(LxError::ENOTCONN);
        //     }
        // }
        unimplemented!()
    }

    fn icmp_write(&self,data:&[u8],remote_addr: IpAddress) -> LxResult<usize> {
        unimplemented!()
    }

    fn poll(&self) -> (bool, bool, bool) {
        unimplemented!()
    }

    fn connect(&mut self, endpoint: Endpoint) -> LxResult<usize> {
        unimplemented!()
    }

    fn bind(&mut self, _endpoint: Endpoint) -> LxResult<usize> {
        let mut sockets = SOCKETS.lock();
        let mut socket = sockets.get::<IcmpSocket>(self.handle.0);
        if !socket.is_open() {
            socket.bind(IcmpEndpoint::Ident(0x1234)).unwrap();
            // send_at = timestamp;
        }
        unimplemented!()
    }

    fn ioctl(&mut self, request: usize, arg1: usize, _arg2: usize, _arg3: usize) -> LxResult<usize> {
        unimplemented!()
    }

    fn endpoint(&self) -> Option<Endpoint> {
        unimplemented!()
    }

    fn remote_endpoint(&self) -> Option<Endpoint> {
        unimplemented!()
    }
}
impl_kobject!(IcmpSocketState);




#[async_trait]
impl FileLike for IcmpSocketState {
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
}
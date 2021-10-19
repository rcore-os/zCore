// icmpsocket
#![allow(dead_code)]
// crate
use crate::net::get_net_sockets;
use crate::net::Endpoint;
use crate::net::GlobalSocketHandle;
use crate::net::IpAddress;
use crate::net::LxResult;
use crate::net::Socket;
use crate::net::SysResult;
use crate::net::ICMP_METADATA_BUF;
use crate::net::ICMP_RECVBUF;
use crate::net::ICMP_SENDBUF;
use alloc::sync::Arc;
use spin::Mutex;

// alloc
use alloc::boxed::Box;
use alloc::vec;

// smoltcp

use smoltcp::socket::IcmpPacketMetadata;
use smoltcp::socket::IcmpSocket;
use smoltcp::socket::IcmpSocketBuffer;

// async
use async_trait::async_trait;

// third part
use zircon_object::impl_kobject;
use zircon_object::object::*;

/// missing documentation
pub struct IcmpSocketState {
    /// missing documentation
    base: KObjectBase,
    /// missing documentation
    handle: GlobalSocketHandle,
}

impl Default for IcmpSocketState {
    fn default() -> Self {
        Self::new()
    }
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
        let handle = GlobalSocketHandle(get_net_sockets().lock().add(socket));

        IcmpSocketState {
            base: KObjectBase::new(),
            handle,
        }
    }

    /// missing documentation
    pub async fn read(&self, _data: &mut [u8]) -> SysResult {
        unimplemented!()
    }

    fn write(&self, _data: &[u8], _remote_addr: IpAddress) -> SysResult {
        unimplemented!()
    }

    fn poll(&self) -> (bool, bool, bool) {
        unimplemented!()
    }

    fn connect(&mut self, _endpoint: Endpoint) -> SysResult {
        unimplemented!()
    }

    fn bind(&mut self, _endpoint: Endpoint) -> SysResult {
        unimplemented!()
    }

    fn ioctl(&mut self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
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
impl Socket for IcmpSocketState {
    /// read to buffer
    async fn read(&self, _data: &mut [u8]) -> (SysResult, Endpoint) {
        unimplemented!()
        // self.read(data).await
    }
    /// write from buffer
    fn write(&self, _data: &[u8], _sendto_endpoint: Option<Endpoint>) -> SysResult {
        unimplemented!()
        // self.write(data, sendto_endpoint)
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
    fn setsockopt(&mut self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        unimplemented!()
        // self.setsockopt(level, opt, data)
    }

    /// manipulate file descriptor
    fn ioctl(&self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> SysResult {
        Ok(0)
    }

    fn fcntl(&self, _cmd: usize, _arg: usize) -> SysResult {
        Ok(0)
    }
}

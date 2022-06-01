use super::*;

use linux_object::net::sockaddr_to_endpoint;
use linux_object::net::SockAddr;
use linux_object::net::Socket;
use linux_object::net::TcpSocketState;
use linux_object::net::UdpSocketState;
use linux_object::net::NetlinkSocketState;
use linux_object::net::MsgHdr;
use kernel_hal::user::{UserInOutPtr, IoVecs};

use spin::Mutex;

impl Syscall<'_> {
    /// net socket
    pub fn sys_socket(&mut self, domain: usize, socket_type: usize, protocol: usize) -> SysResult {
        info!(
            "sys_socket: domain: {:?}, socket_type: {:?}, protocol: {}",
            domain, socket_type, protocol
        );
        let proc = self.linux_process();
        let socket: Arc<Mutex<dyn Socket>> = match domain {
            //     musl
            //     domain local 1
            //     domain inet  2
            //     domain inet6 10
            //     domain netlink 16
            2 | 1 => match socket_type {
                //         musl socket type
                //              1 STREAM
                //              2 DGRAM
                //              3 RAW
                //              4 RDM
                //              5 SEQPACKET
                //              5 SEQPACKET
                //              6 DCCP
                //              10 SOCK_PACKET
                1 => Arc::new(Mutex::new(TcpSocketState::new())),
                2 => Arc::new(Mutex::new(UdpSocketState::new())),
                3 => match protocol {
                    1 => Arc::new(Mutex::new(UdpSocketState::new())),
                    _ => Arc::new(Mutex::new(UdpSocketState::new())),
                },
                _ => return Err(LxError::EINVAL),
            },
            // AF_NETLINK
            16 => {
                match socket_type {
                    3 => Arc::new(Mutex::new(NetlinkSocketState::new())),
                    _ => return Err(LxError::EINVAL),
                }
            }
            _ => return Err(LxError::EAFNOSUPPORT),
        };
        // socket
        let fd = proc.add_socket(socket)?;
        Ok(fd.into())
    }

    /// net sys_connect
    pub async fn sys_connect(
        &mut self,
        fd: usize,
        addr: UserInPtr<SockAddr>,
        addr_len: usize,
    ) -> SysResult {
        info!(
            "sys_connect: fd: {}, addr: {:?}, addr_len: {}",
            fd, addr, addr_len
        );

        let mut _proc = self.linux_process();
        let sa: SockAddr = addr.read()?;

        let endpoint = sockaddr_to_endpoint(sa, addr_len)?;
        let socket = _proc.get_socket(fd.into())?;
        let x = socket.lock();
        x.connect(endpoint).await?;
        Ok(0)
    }

    /// net setsockopt
    pub fn sys_setsockopt(
        &mut self,
        sockfd: usize,
        level: usize,
        optname: usize,
        optval: UserInPtr<u8>,
        optlen: usize,
    ) -> SysResult {
        info!(
            "sys_setsockopt : sockfd : {:?}, level : {:?}, optname : {:?}, optval : {:?} , optlen : {:?}",
            sockfd, level, optname,optval,optlen
        );
        self.linux_process()
            .get_socket(sockfd.into())?
            .lock()
            .setsockopt(level, optname, optval.as_slice(optlen)?)
    }

    /// net getsockopt
    pub fn sys_getsockopt(
        &mut self,
        sockfd: usize,
        level: usize,
        optname: usize,
        optval: UserOutPtr<u8>,
        optlen: usize,
    ) -> SysResult {
        info!(
            "sys_getsockopt : sockfd : {:?}, level : {:?}, optname : {:?}, optval : {:?} , optlen : {:?}",
            sockfd, level, optname,optval,optlen
        );
        Ok(0)
    }

    /// net setsockopt
    pub fn sys_sendto(
        &mut self,
        sockfd: usize,
        buffer: UserInPtr<u8>,
        length: usize,
        flags: usize,
        dest_addr: UserInPtr<SockAddr>,
        addrlen: usize,
    ) -> SysResult {
        info!(
            "sys_sendto : sockfd : {:?}, buffer : {:?}, length : {:?}, flags : {:?} , dest_addr : {:?}, addrlen : {:?}",
            sockfd,buffer,length,flags,dest_addr,addrlen
        );
        let endpoint = if dest_addr.is_null() {
            None
        } else {
            let _sa: SockAddr = dest_addr.read()?;
            let endpoint = sockaddr_to_endpoint(dest_addr.read()?, addrlen)?;
            Some(endpoint)
        };
        let proc = self.linux_process();
        let socket = proc.get_socket(sockfd.into())?;
        let len = socket.lock().write(buffer.as_slice(length)?, endpoint)?;
        Ok(len)
    }

    /// net setsockopt
    pub async fn sys_recvfrom(
        &mut self,
        sockfd: usize,
        mut buffer: UserOutPtr<u8>,
        length: usize,
        flags: usize,
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_recvfrom : sockfd : {:?}, buffer : {:?}, length : {:?}, flags : {:?} , optlen : {:?}, addr_len : {:?}",
            sockfd, buffer, length,flags,addr,addr_len
        );
        let proc = self.linux_process();
        let mut data = vec![0u8; length];
        let socket = proc.get_socket(sockfd.into())?;
        let x = socket.lock();
        let (result, endpoint) = x.read(&mut data).await;
        if result.is_ok() && !addr.is_null() {
            let sockaddr_in = SockAddr::from(endpoint);
            sockaddr_in.write_to(addr, addr_len)?;
        }
        buffer.write_array(&data[..length])?;
        result
    }

    /// net recvmsg
    pub async fn sys_recvmsg(
        &mut self,
        sockfd: usize,
        msg:UserInOutPtr<MsgHdr>,
        flags: usize,
    ) -> SysResult {
        info!(
            "sys_recvmsg :sockfd : {:?}, msg : {:?}, flags : {:?}",
            sockfd, msg, flags
        );
        let hdr = msg.read().unwrap();

        let mut iov_ptr = hdr.msg_iov;
        let iovlen = hdr.msg_iovlen;
        let mut iovs = IoVecs::new(iov_ptr, iovlen);
        let mut data = vec![0u8; 8192];


        let proc = self.linux_process();
        let socket = proc.get_socket(sockfd.into())?;
        let x = socket.lock();
        let (result, endpoint) = x.read(&mut data).await;


        let addr = hdr.msg_name;
        if result.is_ok() && !addr.is_null() {
            iovs.write_from_buf(&data).unwrap();
            let sockaddr_in = SockAddr::from(endpoint);
            sockaddr_in.write_to_msg(msg)?;
        }

        result
    }

    /// net bind
    pub fn sys_bind(&mut self, fd: usize, addr: UserInPtr<SockAddr>, addr_len: usize) -> SysResult {
        info!("sys_bind: fd={:?} addr={:?} len={}", fd, addr, addr_len);
        let proc = self.linux_process();
        let sa: SockAddr = addr.read()?;
        let endpoint = sockaddr_to_endpoint(sa, addr_len)?;
        info!("sys_bind: fd={:?} bind to {:?}", fd, endpoint);

        let socket = proc.get_socket(fd.into())?;
        let mut x = socket.lock();
        x.bind(endpoint)
    }

    /// net listen
    pub fn sys_listen(&mut self, fd: usize, backlog: usize) -> SysResult {
        info!("sys_listen: fd={:?} backlog={}", fd, backlog);
        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let proc = self.linux_process();

        let socket = proc.get_socket(fd.into())?;
        let mut x = socket.lock();
        x.listen()
    }

    /// net shutdown
    pub fn sys_shutdown(&mut self, fd: usize, how: usize) -> SysResult {
        info!("sys_shutdown: fd={:?} how={}", fd, how);
        let proc = self.linux_process();

        let socket = proc.get_socket(fd.into())?;
        let x = socket.lock();
        x.shutdown()
    }

    /// net accept
    pub async fn sys_accept(
        &mut self,
        fd: usize,
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_accept: fd={:?} addr={:?} addr_len={:?}",
            fd, addr, addr_len
        );
        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let proc = self.linux_process();

        let socket = proc.get_socket(fd.into())?;
        let (new_socket, remote_endpoint) = socket.lock().accept().await?;
        let new_fd = proc.add_socket(new_socket)?;

        if !addr.is_null() {
            let sockaddr_in = SockAddr::from(remote_endpoint);
            sockaddr_in.write_to(addr, addr_len)?;
        }
        Ok(new_fd.into())
    }

    /// net getsocknames
    pub fn sys_getsockname(
        &mut self,
        fd: usize,
        addr: UserInOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_getsockname: fd={:?} addr={:?} addr_len={:?}",
            fd, addr, addr_len
        );
        if addr.is_null() {
            return Err(LxError::EINVAL);
        }
        let endpoint = sockaddr_to_endpoint(addr.read()?, addr_len.read()? as usize).unwrap();
        let sockaddr_in = SockAddr::from(endpoint);
        sockaddr_in.write_to_inout(addr, addr_len)?;
        Ok(0)
    }

    /// net getpeername
    pub fn sys_getpeername(
        &mut self,
        fd: usize,
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_getpeername: fd={:?} addr={:?} addr_len={:?}",
            fd, addr, addr_len
        );

        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let proc = self.linux_process();

        if addr.is_null() {
            return Err(LxError::EINVAL);
        }

        let socket = proc.get_socket(fd.into())?;
        let remote_endpoint = socket.lock().remote_endpoint().ok_or(LxError::EINVAL)?;
        let sockaddr_in = SockAddr::from(remote_endpoint);
        sockaddr_in.write_to(addr, addr_len)?;
        Ok(0)
    }
}

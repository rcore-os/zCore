use super::*;
use linux_object::net::Socket;
use linux_object::net::UdpSocketState;
use spin::Mutex;
// use linux_object::fs::FileLike;
// use linux_object::fs::FileLikeType;
use linux_object::net::sockaddr_to_endpoint;
// use linux_object::net::RawSocketState;
use linux_object::net::SockAddr;
use linux_object::net::TcpSocketState;
// use linux_object::net::UdpSocketState;

impl Syscall<'_> {
    /// net socket
    pub fn sys_socket(&mut self, domain: usize, socket_type: usize, protocol: usize) -> SysResult {
        // let domain = AddressFamily::from(domain as u16);
        // let socket_type = SocketType::from(socket_type as u8 & SOCK_TYPE_MASK);
        warn!(
            "sys_socket: domain: {:?}, socket_type: {:?}, protocol: {}",
            domain, socket_type, protocol
        );
        let proc = self.linux_process();
        let socket: Arc<Mutex<dyn Socket>> = match domain {
            //     musl
            //     domain local 1
            //     domain inet  2
            //     domain inet6 10
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
                1 => {
                    warn!("TCP");
                    Arc::new(Mutex::new(TcpSocketState::new()))
                    // Arc::new(UdpSocketState::new())
                }
                2 => {
                    warn!("UDP");
                    Arc::new(Mutex::new(UdpSocketState::new()))
                    // Arc::new(UdpSocketState::new())
                }
                3 => match protocol {
                    // 1 => {
                    //     warn!("yes Icmp socekt");
                    //     Arc::new(IcmpSocketState::new())
                    // Arc::new(UdpSocketState::new())
                    // }
                    _ => {
                        warn!("yes Raw socekt");
                        // Arc::new(RawSocketState::new(protocol as u8))
                        Arc::new(Mutex::new(UdpSocketState::new()))
                    }
                },
                _ => return Err(LxError::EINVAL),
            },
            //     AddressFamily::Packet => match socket_type {
            //         SocketType::Raw => Box::new(PacketSocketState::new()),
            //         _ => return Err(SysError::EINVAL),
            //     },
            //     AddressFamily::Netlink => match socket_type {
            //         SocketType::Raw => Box::new(NetlinkSocketState::new()),
            //         _ => return Err(SysError::EINVAL),
            //     },
            _ => return Err(LxError::EAFNOSUPPORT),
        };
        // socket
        let fd = proc.add_socket(socket)?;
        warn!("socketfd : {:?}", fd);
        Ok(fd.into())
        // Ok(0)
    }

    /// net sys_connect
    pub async fn sys_connect(
        &mut self,
        fd: usize,
        addr: UserInPtr<SockAddr>,
        addr_len: usize,
    ) -> SysResult {
        warn!(
            "sys_connect: fd: {}, addr: {:?}, addr_len: {}",
            fd, addr, addr_len
        );

        let mut _proc = self.linux_process();
        let sa: SockAddr = addr.read()?;

        let endpoint = sockaddr_to_endpoint(sa, addr_len)?;
        warn!("connect endpoint : {:?}", endpoint);
        let socket = _proc.get_socket(fd.into())?;
        let x = socket.lock();
        x.connect(endpoint).await?;
        // match file.file_type()? {
        //     FileLikeType::RawSocket => {
        //         unreachable!()
        //     }
        //     FileLikeType::TcpSocket => {
        //         let socket = file
        //             .downcast_arc::<TcpSocketState>()
        //             .map_err(|_| LxError::EBADF)?;
        //         socket.connect(endpoint)?;
        //     }
        //     FileLikeType::UdpSocket => {
        //         unreachable!()
        //     }
        //     _ => unreachable!(),
        // };
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
        warn!(
            "sys_setsockopt : sockfd : {:?}, level : {:?}, optname : {:?}, optval : {:?} , optlen : {:?}",
            sockfd, level, optname,optval,optlen
        );
        let proc = self.linux_process();
        let data = optval.read_array(optlen)?;
        let socket = proc.get_socket(sockfd.into())?;
        let len = socket.lock().setsockopt(level, optname, &data)?;
        Ok(len)
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
        warn!(
            "sys_sendto : sockfd : {:?}, buffer : {:?}, length : {:?}, flags : {:?} , optlen : {:?}, addrlen : {:?}",
            sockfd,buffer,length,flags,dest_addr,addrlen
        );
        let proc = self.linux_process();
        let data = buffer.read_array(length)?;
        let endpoint = if dest_addr.is_null() {
            None
        } else {
            let _sa: SockAddr = dest_addr.read()?;
            let endpoint = sockaddr_to_endpoint(dest_addr.read()?, addrlen)?;
            warn!("sys_sendto: sending to endpoint {:?}", endpoint);
            Some(endpoint)
        };
        // 有问题 FIXME
        let socket = proc.get_socket(sockfd.into())?;
        let len = socket.lock().write(&data, endpoint)?;
        // let len = match file.file_type()? {
        //     FileLikeType::RawSocket => {
        //         let socket = file
        //             .downcast_arc::<RawSocketState>()
        //             .map_err(|_| LxError::EBADF)?;
        //         socket.raw_write(&data, endpoint)?
        //     }
        //     FileLikeType::TcpSocket => {
        //         let socket = file
        //             .downcast_arc::<TcpSocketState>()
        //             .map_err(|_| LxError::EBADF)?;
        //         socket.tcp_write(&data, endpoint)?
        //     }
        //     FileLikeType::UdpSocket => {
        //         // let socket = file
        //         //     .downcast_arc::<UdpSocketState>()
        //         //     .map_err(|_| LxError::EBADF)?;
        //         // socket.udp_write(&data, endpoint)?
        //         0
        //     }
        //     _ => unreachable!(),
        // };
        // warn!("len : {}", len);
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
        warn!(
            "sys_recvfrom : sockfd : {:?}, buffer : {:?}, length : {:?}, flags : {:?} , optlen : {:?}, addr_len : {:?}",
            sockfd, buffer, length,flags,addr,addr_len
        );
        let proc = self.linux_process();
        let mut data = vec![0u8; length];
        // 有问题 FIXME
        let socket = proc.get_socket(sockfd.into())?;
        let x = socket.lock();
        let (result, endpoint) = x.read(&mut data).await;
        if result.is_ok() && !addr.is_null() {
            let sockaddr_in = SockAddr::from(endpoint);
            // #[allow(unsafe_code)]
            //unsafe {
            sockaddr_in.write_to(addr, addr_len)?;
            //}
        }
        buffer.write_array(&data[..length])?;
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
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_getsockname: fd={:?} addr={:?} addr_len={:?}",
            fd, addr, addr_len
        );

        let proc = self.linux_process();

        if addr.is_null() {
            return Err(LxError::EINVAL);
        }

        let socket = proc.get_socket(fd.into())?;
        let endpoint = socket.lock().endpoint().ok_or(LxError::EINVAL)?;
        let sockaddr_in = SockAddr::from(endpoint);
        sockaddr_in.write_to(addr, addr_len)?;
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

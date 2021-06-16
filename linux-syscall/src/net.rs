use linux_object::fs::FileLike;
use linux_object::fs::FileLikeType;
use linux_object::net::sockaddr_to_endpoint;
use linux_object::net::UdpSocketState;
use linux_object::net::TcpSocketState;
use linux_object::net::RawSocketState;
use super::*;
use linux_object::net::SockAddr;

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
        let socket: Arc<dyn FileLike> = match domain {
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
                    Arc::new(TcpSocketState::new())
                }
                2 => {
                    warn!("UDP");
                    Arc::new(UdpSocketState::new())
                    // Arc::new(UdpSocketState::new())
                }
                3 => match protocol {
                    // 1 => {
                    //     warn!("yes Icmp socekt");
                    //     Arc::new(IcmpSocketState::new())
                    // }
                    _ => {
                        warn!("yes Raw socekt");
                        Arc::new(RawSocketState::new(protocol as u8))
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
        let fd = proc.add_file(socket)?;
        Ok(fd.into())
    }

    /// net sys_connect
    pub fn sys_connect(
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
        // 有问题 FIXME
        let file = _proc.get_file_like(fd.into())?;
        match file.file_type()? {
            FileLikeType::RawSocket => {
                unreachable!()
            },
            FileLikeType::TcpSocket => {
                let socket = file.downcast_arc::<TcpSocketState>().map_err(|_| LxError::EBADF)?;
                socket.connect(endpoint)?;
            },
            FileLikeType::UdpSocket => {
                unreachable!()
            },
            _ => unreachable!()
        };
        Ok(0)
    }

    /// net setsockopt
    pub fn sys_setsockopt(
        &mut self,
        sockfd: usize,
        level: usize,
        optname: usize,
        optval: UserInPtr<i32>,
        optlen: usize,
    ) -> SysResult {
        warn!(
            "sys_setsockopt : sockfd : {:?}, level : {:?}, optname : {:?}, optval : {:?} , optlen : {:?}",
            sockfd, level, optname,optval,optlen
        );
        let proc = self.linux_process();
        let _data = optval.read_array(optlen)?;
        let file = proc.get_file_like(sockfd.into())?;
        let len = match file.file_type()? {
            FileLikeType::RawSocket => {
                // let socket = file.downcast_arc::<RawSocketState>().map_err(|_| LxError::EBADF)?;
                // socket.raw_setsockopt(level, optname, data);
                0
            },
            FileLikeType::TcpSocket => {
                unreachable!()
            },
            FileLikeType::UdpSocket => {
                unreachable!()
            },
            _ => unreachable!()
        };
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
        let file = proc.get_file_like(sockfd.into())?;
        let len = match file.file_type()? {
            FileLikeType::RawSocket => {
                let socket = file.downcast_arc::<RawSocketState>().map_err(|_| LxError::EBADF)?;
                socket.raw_write(&data, endpoint)?
            },
            FileLikeType::TcpSocket => {
                let socket = file.downcast_arc::<TcpSocketState>().map_err(|_| LxError::EBADF)?;
                socket.tcp_write(&data, endpoint)?
            },
            FileLikeType::UdpSocket => {
                let socket = file.downcast_arc::<UdpSocketState>().map_err(|_| LxError::EBADF)?;
                socket.udp_write(&data, endpoint)?
            },
            _ => unreachable!()
        };
        Ok(len)

    }

    /// net setsockopt
    pub fn sys_recvfrom(
        &mut self,
        sockfd: usize,
        mut buffer: UserOutPtr<u8>,
        length: usize,
        flags: usize,
        src_addr: UserOutPtr<SockAddr>,
        addrlen: usize,
    ) -> SysResult {
        warn!(
            "sys_recvfrom : sockfd : {:?}, buffer : {:?}, length : {:?}, flags : {:?} , optlen : {:?}, addrlen : {:?}",
            sockfd, buffer, length,flags,src_addr,addrlen
        );
        let proc = self.linux_process();
        let mut data = vec![0u8; length];
        // 有问题 FIXME
        let file = proc.get_file_like(sockfd.into())?;
        let (result, endpoint) = match file.file_type()? {
            FileLikeType::RawSocket => {
                let socket = file.downcast_arc::<RawSocketState>().map_err(|_| LxError::EBADF)?;
                socket.raw_read(&mut data)
                
            },
            FileLikeType::TcpSocket => {
                let socket = file.downcast_arc::<TcpSocketState>().map_err(|_| LxError::EBADF)?;
                socket.tcp_read(&mut data)
            },
            FileLikeType::UdpSocket => {
                let socket = file.downcast_arc::<UdpSocketState>().map_err(|_| LxError::EBADF)?;
                socket.udp_read(&mut data)
            },
            _ => unreachable!()
        };
        if result.is_ok() && !src_addr.is_null() {
            let _sockaddr_in = SockAddr::from(endpoint);
            #[allow(unsafe_code)]
            unsafe {
                _sockaddr_in.write_to()?;
            }
        }
        buffer.write_array(&data[..length])?;
        result
    }
}

use linux_object::fs::FileLike;
use linux_object::net::sockaddr_to_endpoint;
use linux_object::net::UdpSocketState;
use linux_object::net::TcpSocketState;
// use linux_object::net::RawSocketState;
use super::*;
use linux_object::net::SockAddr;

impl Syscall<'_> {
    /// net socket
    pub fn sys_socket(&mut self, domain: usize, socket_type: usize, _protocol: usize) -> SysResult {
        // let domain = AddressFamily::from(domain as u16);
        // let socket_type = SocketType::from(socket_type as u8 & SOCK_TYPE_MASK);
        warn!(
            "sys_socket: domain: {:?}, socket_type: {:?}, protocol: {}",
            domain, socket_type, _protocol
        );
        let mut _proc = self.linux_process();
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
                // 3 => match _protocol {
                //     1 => {
                //         warn!("yes Icmp socekt");
                //         Arc::new(IcmpSocketState::new())
                //     }
                //     _ => {
                //         warn!("yes Raw socekt");
                //         Arc::new(RawSocketState::new(_protocol as u8))
                //     }
                // },
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
        let _fd = _proc.add_file(socket)?;
        Ok(_fd.into())
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
        let socket = _proc.get_tcp_socket(fd.into())?;
        warn!("-");
        socket.connect(endpoint)?;
        warn!("--");
        Ok(0)
    }

    /// net setsockopt
    pub fn sys_setsockopt(
        &mut self,
        _sockfd: usize,
        _level: usize,
        _optname: usize,
        _optval: UserInPtr<i32>,
        _optlen: usize,
    ) -> SysResult {
        warn!(
            "sys_setsockopt : _sockfd : {:?}, _level : {:?}, _optname : {:?}, _optval : {:?} , _optlen : {:?}",
            _sockfd, _level, _optname,_optval,_optlen
        );
        let _proc = self.linux_process();
        let _data = _optval.read_array(_optlen)?;
        // let _socket = proc.get_tcp_socket(_sockfd.into())?;
        // _socket.setsockopt(_level, _optname, _data);
        // // if let Some(s) = socket.as_socket() {
        // //     s.setsockopt(_level, _optname, _data);
        // // }
        // Ok(0)
        unimplemented!()
    }

    /// net setsockopt
    pub fn sys_sendto(
        &mut self,
        _sockfd: usize,
        _buffer: UserInPtr<u8>,
        _length: usize,
        _flags: usize,
        _dest_addr: UserInPtr<SockAddr>,
        _addrlen: usize,
    ) -> SysResult {
        warn!(
            "sys_sendto : _sockfd : {:?}, _buffer : {:?}, _length : {:?}, _flags : {:?} , _optlen : {:?}, _addrlen : {:?}",
            _sockfd, _buffer, _length,_flags,_dest_addr,_addrlen
        );
        let proc = self.linux_process();
        let _data = _buffer.read_array(_length)?;
        let _endpoint = if _dest_addr.is_null() {
            None
        } else {
            // let endpoint = LxError(&mut self.vm(), addr, addr_len)?;
            // info!("sys_sendto: sending to endpoint {:?}", endpoint);
            // Some(endpoint)
            let _sa: SockAddr = _dest_addr.read()?;
            // Some(endpoint)
            let endpoint = sockaddr_to_endpoint(_dest_addr.read()?, _addrlen)?;
            warn!("sys_sendto: sending to endpoint {:?}", endpoint);
            Some(endpoint)
        };
        // 有问题 FIXME
        let _socket = proc.get_tcp_socket(_sockfd.into())?;
        // let s = _socket.as_socket().unwrap();
        warn!("data : {:?}", _data);
        let a = _socket.tcp_write(&_data, _endpoint)?;
        warn!("a : {:?}", a);
        // let a = _socket.write_sk(&_data, _endpoint.unwrap())?;
        // _socket.write(&slice, endpoint)
        Ok(a)
        // unimplemented!()
    }

    /// net setsockopt
    pub fn sys_recvfrom(
        &mut self,
        _sockfd: usize,
        mut _buffer: UserOutPtr<u8>,
        _length: usize,
        _flags: usize,
        _src_addr: UserOutPtr<SockAddr>,
        _addrlen: usize,
    ) -> SysResult {
        warn!(
            "sys_recvfrom : _sockfd : {:?}, _buffer : {:?}, _length : {:?}, _flags : {:?} , _optlen : {:?}, _addrlen : {:?}",
            _sockfd, _buffer, _length,_flags,_src_addr,_addrlen
        );
        let proc = self.linux_process();
        let mut data = vec![0u8; _length];
        // // warn!("loop1");
        // 有问题 FIXME
        let _socket = proc.get_tcp_socket(_sockfd.into())?;
        // // warn!("loop2");
        let (result, endpoint) = _socket.tcp_read(&mut data);
        // // warn!("loop3");
        _buffer.write_array(&data[.._length])?;
        // // warn!("loop4");
        if result.is_ok() && !_src_addr.is_null() {
            let _sockaddr_in = SockAddr::from(endpoint);
            #[allow(unsafe_code)]
            unsafe {
                _sockaddr_in.write_to()?;
            }
        }
        // // // _socket.send();
        // // let mut data = vec![0u8; _length];
        // // let (result, _endpoint) = _socket.read_sk(&mut data)?;
        // // _buffer.write_array(&data[.._length])?;
        // // // if result.is_ok() && !addr.is_null() {
        // // //     let sockaddr_in = SockAddr::from(endpoint);
        // // //     unsafe {
        // // //         sockaddr_in.write_to(&mut self.vm(), addr, addr_len)?;
        // // //     }
        // // // }
        // // // error!("result : {}",result);
        // // Ok(result)
        // // warn!("result : {:?}",result);
        result
    }
}

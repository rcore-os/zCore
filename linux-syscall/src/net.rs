use core::intrinsics::size_of;

use super::*;

use kernel_hal::user::{IoVecs, UserInOutPtr};
use linux_object::net::sockaddr_to_endpoint;
use linux_object::net::MsgHdr;
use linux_object::net::NetlinkSocketState;
use linux_object::net::SockAddr;
use linux_object::net::Socket;
use linux_object::net::TcpSocketState;
use linux_object::net::UdpSocketState;
use linux_object::net::{Domain, IpOptname, Level, Protocol, SocketType, SolOptname, TcpOptname};
use linux_object::net::{TCP_RECVBUF, TCP_SENDBUF};
use lock::Mutex;

impl Syscall<'_> {
    /// creates an endpoint for communication and returns a file descriptor that refers to that endpoint.
    pub fn sys_socket(&mut self, domain: usize, _type: usize, protocol: usize) -> SysResult {
        info!(
            "sys_socket: domain:{}, type:{}, protocol:{}",
            domain, _type, protocol
        );
        let domain = match Domain::try_from(domain) {
            Ok(domain) => domain,
            Err(_) => {
                error!("invalid domain: {domain}");
                return Err(LxError::EAFNOSUPPORT);
            }
        };
        let _type = match SocketType::try_from(_type) {
            Ok(_type) => _type,
            Err(_) => {
                // todo: 加上打印会导致测试用例不过
                //error!("invalid type: {_type}");
                return Err(LxError::EINVAL);
            }
        };
        let protocol = match Protocol::try_from(protocol) {
            Ok(protocol) => protocol,
            Err(_) => {
                error!("invalid protocol: {protocol}");
                return Err(LxError::EINVAL);
            }
        };
        let socket: Arc<Mutex<dyn Socket>> = match domain {
            Domain::AF_LOCAL | Domain::AF_INET => match _type {
                SocketType::SOCK_STREAM => Arc::new(Mutex::new(TcpSocketState::new())),
                SocketType::SOCK_DGRAM => Arc::new(Mutex::new(UdpSocketState::new())),
                SocketType::SOCK_RAW => match protocol {
                    // todo 需要实现针对不同protocol的处理
                    Protocol::IPPROTO_ICMP => Arc::new(Mutex::new(UdpSocketState::new())),
                    _ => Arc::new(Mutex::new(UdpSocketState::new())),
                },
                _ => return Err(LxError::EINVAL),
            },
            Domain::AF_INET6 => return Err(LxError::EAFNOSUPPORT),
            Domain::AF_NETLINK => match _type {
                SocketType::SOCK_RAW => Arc::new(Mutex::new(NetlinkSocketState::default())),
                _ => return Err(LxError::EINVAL),
            },
        };
        // socket
        let fd = self.linux_process().add_socket(socket)?;
        Ok(fd)
    }

    ///  connects the socket referred to by the file descriptor sockfd to the address specified by addr.
    pub async fn sys_connect(
        &mut self,
        sockfd: usize,
        addr: UserInPtr<SockAddr>,
        addrlen: usize,
    ) -> SysResult {
        info!(
            "sys_connect: sockfd:{}, addr:{:?}, addrlen:{}",
            sockfd, addr, addrlen
        );
        let endpoint = sockaddr_to_endpoint(addr.read()?, addrlen)?;
        self.linux_process()
            .get_socket(sockfd)?
            .lock()
            .connect(endpoint)
            .await?;
        Ok(0)
    }

    /// set options for the socket referred to by the file descriptor sockfd.
    pub fn sys_setsockopt(
        &mut self,
        sockfd: usize,
        level: usize,
        optname: usize,
        optval: UserInPtr<u8>,
        optlen: usize,
    ) -> SysResult {
        info!(
            "sys_setsockopt: sockfd:{}, level:{}, optname:{}, optval:{:?} , optlen:{}",
            sockfd, level, optname, optval, optlen
        );
        self.linux_process().get_socket(sockfd)?.lock().setsockopt(
            level,
            optname,
            optval.as_slice(optlen)?,
        )
    }

    /// get options for the socket referred to by the file descriptor sockfd.
    pub fn sys_getsockopt(
        &mut self,
        sockfd: usize,
        level: usize,
        optname: usize,
        mut optval: UserOutPtr<u32>,
        mut optlen: UserOutPtr<usize>,
    ) -> SysResult {
        info!(
            "sys_getsockopt: sockfd:{}, level:{}, optname:{}, optval:{:?} , optlen:{:?}",
            sockfd, level, optname, optval, optlen
        );
        let level = match Level::try_from(level) {
            Ok(level) => level,
            Err(_) => {
                error!("invalid level: {}", level);
                return Err(LxError::ENOPROTOOPT);
            }
        };
        if optval.is_null() {
            return Err(LxError::EINVAL);
        }
        match level {
            Level::SOL_SOCKET => {
                let optname = match SolOptname::try_from(optname) {
                    Ok(optname) => optname,
                    Err(_) => {
                        error!("invalid optname: {}", optname);
                        return Err(LxError::ENOPROTOOPT);
                    }
                };
                match optname {
                    SolOptname::SNDBUF => {
                        optval.write(TCP_SENDBUF as u32)?;
                        optlen.write(size_of::<u32>())?;
                        Ok(0)
                    }
                    SolOptname::RCVBUF => {
                        optval.write(TCP_RECVBUF as u32)?;
                        optlen.write(size_of::<u32>())?;
                        Ok(0)
                    }
                    _ => Err(LxError::ENOPROTOOPT),
                }
            }
            Level::IPPROTO_TCP => {
                let optname = match TcpOptname::try_from(optname) {
                    Ok(optname) => optname,
                    Err(_) => {
                        error!("invalid optname: {}", optname);
                        return Err(LxError::ENOPROTOOPT);
                    }
                };
                match optname {
                    TcpOptname::CONGESTION => Ok(0),
                }
            }
            Level::IPPROTO_IP => {
                let optname = match IpOptname::try_from(optname) {
                    Ok(optname) => optname,
                    Err(_) => {
                        error!("invalid optname: {}", optname);
                        return Err(LxError::ENOPROTOOPT);
                    }
                };
                match optname {
                    IpOptname::HDRINCL => unimplemented!(),
                }
            }
        }
    }

    /// transmit a message to another socket
    pub fn sys_sendto(
        &mut self,
        sockfd: usize,
        buf: UserInPtr<u8>,
        len: usize,
        flags: usize,
        dest_addr: UserInPtr<SockAddr>,
        addrlen: usize,
    ) -> SysResult {
        info!(
            "sys_sendto: sockfd:{:?}, buffer:{:?}, length:{:?}, flags:{:?} , optlen:{:?}, addrlen:{:?}",
            sockfd, buf, len, flags, dest_addr, addrlen
        );
        let endpoint = if dest_addr.is_null() {
            None
        } else {
            let endpoint = sockaddr_to_endpoint(dest_addr.read()?, addrlen)?;
            Some(endpoint)
        };
        let len = self
            .linux_process()
            .get_socket(sockfd)?
            .lock()
            .write(buf.as_slice(len)?, endpoint)?;
        Ok(len)
    }

    /// receive messages from a socket
    pub async fn sys_recvfrom(
        &mut self,
        sockfd: usize,
        mut buf: UserOutPtr<u8>,
        len: usize,
        flags: usize,
        src_addr: UserOutPtr<SockAddr>,
        addrlen: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_recvfrom: sockfd:{}, buffer:{:?}, length:{}, flags:{} , src_addr:{:?}, addrlen:{:?}",
            sockfd, buf, len, flags, src_addr, addrlen
        );
        let mut data = vec![0u8; len];
        let (result, endpoint) = self
            .linux_process()
            .get_socket(sockfd)?
            .lock()
            .read(&mut data)
            .await;
        if result.is_ok() && !src_addr.is_null() {
            let sockaddr_in = SockAddr::from(endpoint);
            sockaddr_in.write_to(src_addr, addrlen)?;
        }
        buf.write_array(&data[..len])?;
        result
    }

    /// receive messages from a socket
    pub async fn sys_recvmsg(
        &mut self,
        sockfd: usize,
        msg: UserInOutPtr<MsgHdr>,
        flags: usize,
    ) -> SysResult {
        info!(
            "sys_recvmsg: sockfd:{}, msg:{:?}, flags:{}",
            sockfd, msg, flags
        );
        let hdr = msg.read().unwrap();

        let iov_ptr = hdr.msg_iov;
        let iovlen = hdr.msg_iovlen;
        let mut iovs = IoVecs::new(iov_ptr, iovlen);
        let mut data = vec![0u8; 8192];

        let proc = self.linux_process();
        let socket = proc.get_socket(sockfd)?;
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

    /// assigns the address specified by addr to the socket referred to by the file descriptor sockfd
    pub fn sys_bind(
        &mut self,
        sockfd: usize,
        addr: UserInPtr<SockAddr>,
        addrlen: usize,
    ) -> SysResult {
        info!(
            "sys_bind: sockfd:{:?}, addr:{:?}, addrlen:{}",
            sockfd, addr, addrlen
        );
        let endpoint = sockaddr_to_endpoint(addr.read()?, addrlen)?;
        info!("sys_bind: fd:{} bind to {:?}", sockfd, endpoint);
        self.linux_process()
            .get_socket(sockfd)?
            .lock()
            .bind(endpoint)
    }

    /// marks the socket referred to by sockfd as a passive socket,
    /// that is, as a socket that will be used to accept incoming connection
    pub fn sys_listen(&mut self, sockfd: usize, backlog: usize) -> SysResult {
        info!("sys_listen: fd:{:?}, backlog:{}", sockfd, backlog);
        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        self.linux_process().get_socket(sockfd)?.lock().listen()
    }

    /// shutdown a socket
    pub fn sys_shutdown(&mut self, sockfd: usize, howto: usize) -> SysResult {
        info!("sys_shutdown: sockfd:{}, howto:{}", sockfd, howto);
        // todo: how to use 'howto'
        self.linux_process().get_socket(sockfd)?.lock().shutdown()
    }

    /// accept() is used with connection-based socket types (SOCK_STREAM, SOCK_SEQPACKET).
    /// It extracts the first connection request on the queue of pending connections
    /// for the listening socket, sockfd, creates a new connected socket, and returns
    /// a new file descriptor referring to that socket.
    /// The newly created socket is not in the listening state.
    /// The original socket sockfd is unaffected by this call.
    pub async fn sys_accept(
        &mut self,
        sockfd: usize,
        addr: UserOutPtr<SockAddr>,
        addrlen: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_accept: sockfd:{}, addr:{:?}, addrlen={:?}",
            sockfd, addr, addrlen
        );
        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let (new_socket, remote_endpoint) = self
            .linux_process()
            .get_socket(sockfd)?
            .lock()
            .accept()
            .await?;
        let new_fd = self.linux_process().add_socket(new_socket)?;
        if !addr.is_null() {
            let sockaddr_in = SockAddr::from(remote_endpoint);
            sockaddr_in.write_to(addr, addrlen)?;
        }
        Ok(new_fd)
    }

    /// returns the current address to which the socket sockfd is bound,
    /// in the buffer pointed to by addr.
    pub fn sys_getsockname(
        &mut self,
        sockfd: usize,
        addr: UserOutPtr<SockAddr>,
        addrlen: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_getsockname: sockfd:{}, addr:{:?}, addrlen:{:?}",
            sockfd, addr, addrlen
        );
        if addr.is_null() {
            return Err(LxError::EINVAL);
        }
        let endpoint = self
            .linux_process()
            .get_socket(sockfd)?
            .lock()
            .endpoint()
            .ok_or(LxError::EINVAL)?;
        SockAddr::from(endpoint).write_to(addr, addrlen)?;
        Ok(0)
    }

    /// returns  the  address  of the peer connected to the socket sockfd,
    /// in the buffer pointed to by addr.
    pub fn sys_getpeername(
        &mut self,
        sockfd: usize,
        addr: UserOutPtr<SockAddr>,
        addrlen: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_getpeername: sockfd:{}, addr:{:?}, addrlen:{:?}",
            sockfd, addr, addrlen
        );
        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        if addr.is_null() {
            return Err(LxError::EINVAL);
        }
        let remote_endpoint = self
            .linux_process()
            .get_socket(sockfd)?
            .lock()
            .remote_endpoint()
            .ok_or(LxError::EINVAL)?;
        SockAddr::from(remote_endpoint).write_to(addr, addrlen)?;
        Ok(0)
    }
}

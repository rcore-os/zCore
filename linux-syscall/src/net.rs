use super::*;

use linux_object::net::sockaddr_to_endpoint;
use linux_object::net::SockAddr;
use linux_object::net::Socket;
use linux_object::net::TcpSocketState;
use linux_object::net::UdpSocketState;

use spin::Mutex;

impl Syscall<'_> {
    /// Create an endpoint for communication
    ///
    /// socket() creates an endpoint for communication and returns a file
    /// descriptor that refers to that endpoint.  The file descriptor
    /// returned by a successful call will be the lowest-numbered file
    /// descriptor not currently open for the process.
    ///
    /// # argument
    ///
    /// * `domain` - specifies a communication domain.
    /// * `socket_type` - specifies the communication semantics.
    /// * `protocol` - specifies a particular protocol to be used with the socket.
    ///
    /// # return value
    ///
    /// On success, a file descriptor for the new socket is returned.   
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EACCES__ - Permission to create a socket of the specified type and/or protocol is denied.
    /// * __EAFNOSUPPORT__ - The implementation does not support the specified address family.
    /// * __EINVAL__ - Unknown protocol, or protocol family not available.
    /// * __EINVAL__ - Invalid flags in *socket_type*.
    /// * __EMFILE__ - The per-process limit on the number of open file descriptors has been reached.
    /// * __ENFILE__ - The system-wide limit on the total number of open files has been reached.
    /// * __ENOBUFS__ or __ENOMEM__ - Insufficient memory is available.  The socket cannot be created until sufficient resources are freed.
    /// * __EPROTONOSUPPORT__ - The protocol type or the specified protocol is not supported within this domain.
    ///
    pub fn sys_socket(&mut self, domain: usize, socket_type: usize, protocol: usize) -> SysResult {
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
                1 => Arc::new(Mutex::new(TcpSocketState::new())),
                2 => Arc::new(Mutex::new(UdpSocketState::new())),
                3 => match protocol {
                    1 => Arc::new(Mutex::new(UdpSocketState::new())),
                    _ => Arc::new(Mutex::new(UdpSocketState::new())),
                },
                _ => return Err(LxError::EINVAL),
            },
            _ => return Err(LxError::EAFNOSUPPORT),
        };
        // socket
        let fd = proc.add_socket(socket)?;
        Ok(fd.into())
    }

    /// Initiate a connection on a socket
    ///
    /// The connect() system call connects the socket referred to by the
    /// file descriptor sockfd to the address specified by addr.  The
    /// addrlen argument specifies the size of addr.  The format of the
    /// address in addr is determined by the address space of the socket
    /// sockfd.
    ///
    /// # argument
    ///
    /// * `sockfd` - specifies a socket file descriptor.
    /// * `addr` - specifies the address of socket *addr*.
    /// * `addr_len` - specifies the size of *addr* in bytes.
    ///
    /// # return value
    ///
    /// If the connection or binding succeeds, zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - invalid value.
    ///
    pub async fn sys_connect(
        &mut self,
        sockfd: usize,
        addr: UserInPtr<SockAddr>,
        addr_len: usize,
    ) -> SysResult {
        warn!(
            "sys_connect: sockfd: {}, addr: {:?}, addr_len: {}",
            sockfd, addr, addr_len
        );

        let mut _proc = self.linux_process();
        let sa: SockAddr = addr.read()?;

        let endpoint = sockaddr_to_endpoint(sa, addr_len)?;
        let socket = _proc.get_socket(sockfd.into())?;
        let x = socket.lock();
        x.connect(endpoint).await?;
        Ok(0)
    }

    /// Set options on sockets
    ///
    /// setsockopt() manipulate options for the socket
    /// referred to by the file descriptor sockfd.  Options may exist at
    /// multiple protocol levels; they are always present at the
    /// uppermost socket level.
    ///
    /// # argument
    ///
    /// * `sockfd` - specifies a socket file descriptor.
    /// * `level` -  protocol levels , they are always present at the uppermost socket level
    /// * `optname` - are passed uninterpreted to the appropriate protocol module for interpretation.
    /// * `optval` - the option values.
    /// * `optlen` - the size of the value in bytes.
    ///
    /// # return value
    ///
    /// On success, zero is returned for the standard options.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - invalid value.
    ///
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

    /// Send a message on a socket
    ///
    /// The system call sendto() are used to transmit a message to another socket.
    ///
    /// # argument
    ///
    /// * `sockfd` - the file descriptor of the sending socket.
    /// * `buffer` - point to a buffer that data will send.
    /// * `length` - specifies the size of *buffer*.
    /// * `flags` - type of send.
    /// * `dest_addr` - the address of the target.
    /// * `addrlen` - specifies the size of *dest_addr* in bytes.
    ///
    /// # return value
    ///
    /// On success, return the number of bytes sent.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - invalid value.
    ///
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
            Some(endpoint)
        };
        let socket = proc.get_socket(sockfd.into())?;
        let len = socket.lock().write(&data, endpoint)?;
        Ok(len)
    }

    /// Receive a message from a socket
    ///
    /// The recvfrom() call are used to receive messages from a socket.
    ///
    /// # argument
    ///
    /// * `sockfd` - the file descriptor of the sending socket.
    /// * `buffer` - point to a buffer that will receive data.
    /// * `length` - specifies the size of *buffer*.
    /// * `flags` - type of receive.
    /// * `src_addr` - source address of the message.
    /// * `addrlen` - specifies the size of *src_addr* in bytes.
    ///
    /// # return value
    ///
    /// On success, return the number of bytes received.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// When a stream socket peer has performed an orderly shutdown, the return value will be 0 (the traditional "end-of-file" return).
    ///
    /// Datagram sockets in various domains (e.g., the UNIX and Internet domains) permit zero-length datagrams.  When such a datagram is received, the return value is 0.
    ///
    /// The value 0 may also be returned if the requested number of bytes to receive from a stream socket was 0.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - invalid value.
    ///
    pub async fn sys_recvfrom(
        &mut self,
        sockfd: usize,
        mut buffer: UserOutPtr<u8>,
        length: usize,
        flags: usize,
        src_addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_recvfrom : sockfd : {:?}, buffer : {:?}, length : {:?}, flags : {:?} , src_addr : {:?}, addr_len : {:?}",
            sockfd, buffer, length,flags,src_addr,addr_len
        );
        let proc = self.linux_process();
        let mut data = vec![0u8; length];
        let socket = proc.get_socket(sockfd.into())?;
        let x = socket.lock();
        let (result, endpoint) = x.read(&mut data).await;
        if result.is_ok() && !src_addr.is_null() {
            let sockaddr_in = SockAddr::from(endpoint);
            sockaddr_in.write_to(src_addr, addr_len)?;
        }
        buffer.write_array(&data[..length])?;
        result
    }

    /// Bind a name to a socket
    ///
    /// bind() assigns the address specified by addr to the socket referred to by the file descriptor sockfd.
    ///
    /// # argument
    ///
    /// * `sockfd` - specifies a socket file descriptor.
    /// * `addr` - specifies the address of socket *addr*.
    /// * `addr_len` - specifies the size of *addr* in bytes.
    ///
    /// # return value
    ///
    /// On success, zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - invalid value.
    ///
    pub fn sys_bind(
        &mut self,
        sockfd: usize,
        addr: UserInPtr<SockAddr>,
        addr_len: usize,
    ) -> SysResult {
        info!(
            "sys_bind: sockfd={:?} addr={:?} len={}",
            sockfd, addr, addr_len
        );
        let proc = self.linux_process();
        let sa: SockAddr = addr.read()?;
        let endpoint = sockaddr_to_endpoint(sa, addr_len)?;
        info!("sys_bind: sockfd={:?} bind to {:?}", sockfd, endpoint);

        let socket = proc.get_socket(sockfd.into())?;
        let mut x = socket.lock();
        x.bind(endpoint)
    }

    /// Listen for connections on a socket
    ///
    /// listen() marks the socket referred to by sockfd as a passive
    /// socket, that is, as a socket that will be used to accept incoming
    /// connection requests using accept().
    ///
    /// # argument
    ///
    /// * `sockfd` - is a socket file descriptor refers to a socket of type SOCK_STREAM or SOCK_SEQPACKET.
    /// * `backlog` - defines the maximum length to which the queue of pending connections for *sockfd* may grow.
    ///
    /// # return value
    ///
    /// On success, zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - invalid value.
    ///
    pub fn sys_listen(&mut self, sockfd: usize, backlog: usize) -> SysResult {
        info!("sys_listen: sockfd={:?} backlog={}", sockfd, backlog);
        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let proc = self.linux_process();

        let socket = proc.get_socket(sockfd.into())?;
        let mut x = socket.lock();
        x.listen()
    }

    /// Shut down part of a full-duplex connection
    ///
    /// The shutdown() call causes all or part of a full-duplex connection on the socket associated with sockfd to be shut down.
    ///
    /// # argument
    ///
    /// * `sockfd` - specifies a socket file descriptor.
    /// * `how` - how to shutdown.
    ///     1. SHUT_RD - further receptions will be disallowed.
    ///     2. SHUT_WR - further transmissions will be disallowed.
    ///     3. SHUT_RDWR - further receptions and transmissions will be disallowed.
    ///
    /// # return value
    ///
    /// On success, zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - invalid value.
    ///
    pub fn sys_shutdown(&mut self, sockfd: usize, how: usize) -> SysResult {
        info!("sys_shutdown: sockfd={:?} how={}", sockfd, how);
        let proc = self.linux_process();

        let socket = proc.get_socket(sockfd.into())?;
        let x = socket.lock();
        x.shutdown()
    }

    /// Accept a connection on a socket
    ///
    /// The accept() system call is used with connection-based socket types.
    ///
    /// # argument
    ///
    /// * `sockfd` - specifies a socket file descriptor.
    /// * `addr` - specifies the address of socket *addr*.
    /// * `addr_len` - specifies the size of *addr* in bytes.
    ///
    /// # return value
    ///
    /// On success, these system calls return a file descriptor for the accepted socket.  
    /// On error, LxError is returned, and error enum is set to indicate the error,and *addrlen* is left unchanged.
    ///
    /// # errors
    ///
    /// * __EINVAL__ - invalid value.
    ///
    pub async fn sys_accept(
        &mut self,
        sockfd: usize,
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        warn!(
            "sys_accept: sockfd={:?} addr={:?} addr_len={:?}",
            sockfd, addr, addr_len
        );
        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let proc = self.linux_process();

        let socket = proc.get_socket(sockfd.into())?;
        let (new_socket, remote_endpoint) = socket.lock().accept().await?;
        let new_fd = proc.add_socket(new_socket)?;

        if !addr.is_null() {
            let sockaddr_in = SockAddr::from(remote_endpoint);
            sockaddr_in.write_to(addr, addr_len)?;
        }
        Ok(new_fd.into())
    }

    /// Get socket name
    ///
    /// getsockname() returns the current address to which the socket sockfd is bound, in the buffer pointed to by addr.
    ///
    /// # argument
    ///
    /// * `sockfd` - specifies a socket file descriptor.
    /// * `addr` - specifies the address of socket *addr*.
    /// * `addr_len` - specifies the size of *addr* in bytes.
    ///
    /// # return value
    ///
    /// On success, zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EBADF__ - The argument sockfd is not a valid file descriptor.
    /// * __EFAULT__ - The *addr* argument points to memory not in a valid part of the process address space.
    /// * __EINVAL__ - *addrlen* is invalid (e.g., is negative).
    /// * __ENOBUFS__ - Insufficient resources were available in the system to perform the operation.
    /// * __ENOTSOCK__ - The file descriptor sockfd does not refer to a socket.
    ///
    pub fn sys_getsockname(
        &mut self,
        sockfd: usize,
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_getsockname: sockfd={:?} addr={:?} addr_len={:?}",
            sockfd, addr, addr_len
        );

        let proc = self.linux_process();

        if addr.is_null() {
            return Err(LxError::EINVAL);
        }

        let socket = proc.get_socket(sockfd.into())?;
        let endpoint = socket.lock().endpoint().ok_or(LxError::EINVAL)?;
        let sockaddr_in = SockAddr::from(endpoint);
        sockaddr_in.write_to(addr, addr_len)?;
        Ok(0)
    }

    /// Get name of connected peer socket
    ///
    /// getpeername() returns the address of the peer connected to the socket sockfd, in the buffer pointed to by addr.
    ///
    /// # argument
    ///
    /// * `sockfd` - specifies a socket file descriptor.
    /// * `addr` - specifies the address of socket *addr*.
    /// * `addr_len` - specifies the size of *addr* in bytes.
    ///
    /// # return value
    ///
    /// On success, zero is returned.  
    /// On error, LxError is returned, and error enum is set to indicate the error.
    ///
    /// # errors
    ///
    /// * __EBADF__ - The argument sockfd is not a valid file descriptor.
    /// * __EFAULT__ - The *addr* argument points to memory not in a valid part of the process address space.
    /// * __EINVAL__ - *addrlen* is invalid (e.g., is negative).
    /// * __ENOBUFS__ - Insufficient resources were available in the system to perform the operation.
    /// * __ENOTCONN__ - The socket is not connected.
    /// * __ENOTSOCK__ - The file descriptor sockfd does not refer to a socket.
    ///
    pub fn sys_getpeername(
        &mut self,
        sockfd: usize,
        addr: UserOutPtr<SockAddr>,
        addr_len: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_getpeername: sockfd={:?} addr={:?} addr_len={:?}",
            sockfd, addr, addr_len
        );

        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let proc = self.linux_process();

        if addr.is_null() {
            return Err(LxError::EINVAL);
        }

        let socket = proc.get_socket(sockfd.into())?;
        let remote_endpoint = socket.lock().remote_endpoint().ok_or(LxError::EINVAL)?;
        let sockaddr_in = SockAddr::from(remote_endpoint);
        sockaddr_in.write_to(addr, addr_len)?;
        Ok(0)
    }
}

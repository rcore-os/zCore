use super::*;
use alloc::vec::Vec;
use core::convert::TryInto;
use core::mem::size_of;
use kernel_hal::user::UserInOutPtr;
use linux_object::{
    fs::{split_path, FileLike, OpenFlags},
    net::*,
};

const MSG_DONTWAIT: usize = 0x40;
const MSG_PEEK: usize = 0x2;

/// `struct mmsghdr` from `<sys/socket.h>`: one batch entry for
/// `sendmmsg`/`recvmmsg` — a plain `msghdr` plus the per-message transfer
/// count the kernel writes back. Only its layout is used (stride and the
/// `msg_len` offset); the per-entry `msg_hdr` is re-read by the wrapped
/// single-message syscalls straight from user memory.
#[repr(C)]
#[allow(dead_code)]
struct MMsgHdr {
    msg_hdr: MsgHdr,
    msg_len: u32,
    _pad: u32,
}

/// Read a `sockaddr` from user space, honoring the user-supplied `addrlen`.
///
/// `SockAddr` is a union whose alignment (4, coming from the `u32` fields of
/// the IP/netlink variants) is stricter than a C `struct sockaddr_un` (only
/// 2-byte aligned), and whose size (~110 B) is larger than most concrete
/// address structs. Reading it directly with `UserInPtr::<SockAddr>::read()`
/// therefore had two bugs:
///   (a) it rejected perfectly valid 2-byte-aligned `sockaddr_un` pointers with
///       `EFAULT` (the alignment `check()` requires 4-byte alignment) — this is
///       exactly why connecting to the X11 unix socket failed with
///       "unable to connect to X server: Bad address"; and
///   (b) it always read the full union size regardless of `addrlen`, over-
///       reading past a short user buffer that sits near the end of a mapping.
///
/// Copy exactly `addrlen` bytes (capped at the union size) byte-wise, at
/// 1-byte alignment, into a zeroed buffer instead, matching Linux
/// `move_addr_to_kernel` semantics.
#[allow(unsafe_code)]
fn read_sockaddr(addr: usize, addrlen: usize) -> Result<SockAddr, LxError> {
    if addr == 0 {
        return Err(LxError::EFAULT);
    }
    let n = addrlen.min(size_of::<SockAddr>());
    // Zeroed so any address bytes the user did not supply (`addrlen` shorter
    // than the concrete struct) read back as zero, as Linux does.
    let mut storage: SockAddr = unsafe { core::mem::zeroed() };
    if n > 0 {
        let bytes: UserInPtr<u8> = addr.into();
        let src = bytes.read_array(n)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.as_ptr(),
                &mut storage as *mut SockAddr as *mut u8,
                n,
            );
        }
    }
    Ok(storage)
}

/// Copy an option value out to a `getsockopt` caller.
///
/// `optlen` is a value-result argument (like Linux's `optlen`): the kernel must
/// write at most the caller-supplied `*optlen` bytes and then store the true
/// size back. Previously `optlen` was write-only and the input size was ignored,
/// so an option larger than the caller's buffer (e.g. the 12-byte SO_PEERCRED
/// `ucred` written into a 4-byte buffer) overflowed adjacent user memory.
fn write_sockopt_out(
    optval: UserOutPtr<u32>,
    mut optlen: UserInOutPtr<u32>,
    value: &[u8],
) -> SysResult {
    let max = optlen.read()? as usize;
    let n = value.len().min(max);
    if n > 0 {
        let mut dst: UserOutPtr<u8> = optval.as_addr().into();
        dst.write_array(&value[..n])?;
    }
    optlen.write(value.len() as u32)?;
    Ok(0)
}

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
                warn!("sys_socket: invalid domain: {}", domain);
                return Err(LxError::EAFNOSUPPORT);
            }
        };
        let socket_type_val = _type & SOCKET_TYPE_MASK;
        let socket_type = match SocketType::try_from(socket_type_val) {
            Ok(t) => t,
            Err(_) => {
                warn!(
                    "sys_socket: invalid socket type: {:#x} (masked: {:#x})",
                    _type, socket_type_val
                );
                return Err(LxError::EINVAL);
            }
        };
        // socket flags: SOCK_CLOEXEC SOCK_NONBLOCK
        let flags = OpenFlags::from_bits_truncate(_type & !SOCKET_TYPE_MASK);
        let protocol_num = protocol;
        let protocol = Protocol::try_from(protocol_num).ok();

        info!(
            "sys_socket: domain:{:?}, type:{:?}, protocol:{:?}",
            domain, socket_type, protocol
        );

        let socket: Arc<dyn FileLike> = match (domain, socket_type, protocol) {
            (Domain::AF_INET, SocketType::SOCK_STREAM, Some(Protocol::IPPROTO_IP))
            | (Domain::AF_INET, SocketType::SOCK_STREAM, Some(Protocol::IPPROTO_TCP)) => {
                Arc::new(TcpSocketState::new(false)?)
            }
            (Domain::AF_INET6, SocketType::SOCK_STREAM, Some(Protocol::IPPROTO_IP))
            | (Domain::AF_INET6, SocketType::SOCK_STREAM, Some(Protocol::IPPROTO_TCP)) => {
                Arc::new(TcpSocketState::new(true)?)
            }
            (Domain::AF_INET, SocketType::SOCK_DGRAM, Some(Protocol::IPPROTO_IP))
            | (Domain::AF_INET, SocketType::SOCK_DGRAM, Some(Protocol::IPPROTO_UDP)) => {
                Arc::new(UdpSocketState::new(false)?)
            }
            (Domain::AF_INET6, SocketType::SOCK_DGRAM, Some(Protocol::IPPROTO_IP))
            | (Domain::AF_INET6, SocketType::SOCK_DGRAM, Some(Protocol::IPPROTO_UDP)) => {
                Arc::new(UdpSocketState::new(true)?)
            }
            // Linux ping(8) uses SOCK_DGRAM + IPPROTO_ICMP (ping_socket).
            (Domain::AF_INET, SocketType::SOCK_DGRAM, Some(Protocol::IPPROTO_ICMP)) => {
                Arc::new(IcmpSocketState::new(false)?)
            }
            (Domain::AF_INET6, SocketType::SOCK_DGRAM, Some(Protocol::IPPROTO_ICMPV6)) => {
                Arc::new(IcmpSocketState::new(true)?)
            }
            // Be tolerant for AF_INET/AF_INET6 datagram sockets.
            // Some userlands pass unexpected protocol numbers; for DHCP we only need UDP semantics.
            (Domain::AF_INET, SocketType::SOCK_DGRAM, None) => {
                Arc::new(UdpSocketState::new(false)?)
            }
            (Domain::AF_INET6, SocketType::SOCK_DGRAM, None) => {
                Arc::new(UdpSocketState::new(true)?)
            }
            // AF_INET/AF_INET6 raw sockets (some userlands probe these)
            (Domain::AF_INET, SocketType::SOCK_RAW, _) => {
                Arc::new(RawSocketState::new((protocol_num & 0xff) as u8, false)?)
            }
            (Domain::AF_INET6, SocketType::SOCK_RAW, _) => {
                Arc::new(RawSocketState::new((protocol_num & 0xff) as u8, true)?)
            }
            // AF_NETLINK sockets for interface/address discovery (iproute-style)
            (Domain::AF_NETLINK, SocketType::SOCK_RAW, _)
            | (Domain::AF_NETLINK, SocketType::SOCK_DGRAM, _) => {
                Arc::new(NetlinkSocketState::default())
            }
            // AF_PACKET sockets (used by udhcpc for raw ethernet operations)
            (Domain::AF_PACKET, SocketType::SOCK_RAW, _)
            | (Domain::AF_PACKET, SocketType::SOCK_DGRAM, _) => {
                PacketSocketState::new(socket_type, u16::from_be(protocol_num as u16))?
            }
            // AF_UNIX sockets
            (Domain::AF_UNIX, _, _) => {
                let s = UnixSocketState::new();
                // Record our PID so a peer (e.g. seatd) can read it via
                // SO_PEERCRED when it accepts our connection.
                s.set_owner_pid(self.zircon_process().id() as i32);
                s
            }
            (_, _, _) => {
                info!(
                    "sys_socket: unsupported socket type: domain={:?}, type={:?}, protocol={:?}",
                    domain, socket_type, protocol_num
                );
                return Err(LxError::ENOSYS);
            }
        };

        socket.set_flags(flags)?;
        let fd = self.linux_process().add_socket(socket)?; // dyn FileLike
        Ok(fd.into())
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
        let endpoint = sockaddr_to_endpoint(read_sockaddr(addr.as_addr(), addrlen)?, addrlen)?;
        let proc = self.linux_process();
        let file_like = proc.get_file_like(sockfd.into())?;

        if let Endpoint::Unix(path) = &endpoint {
            if let Ok(client) = file_like.clone().downcast_arc::<UnixSocketState>() {
                return match UnixSocketState::lookup(path) {
                    None => Err(LxError::ECONNREFUSED),
                    Some(server) => {
                        if !server.is_listening() {
                            return Err(LxError::ECONNREFUSED);
                        }
                        // Establish the connection now: create the server's end,
                        // wire it to the client, and queue it for accept(). Wiring
                        // at connect time (rather than in accept) lets the client
                        // send its first bytes — e.g. the X11 handshake — before
                        // the server has accepted, instead of getting ENOTCONN.
                        let server_side = UnixSocketState::new();
                        server_side.set_path(server.bound_path());
                        UnixSocketState::connect_pair(&client, &server_side);
                        server.push_accept(server_side);
                        Ok(0)
                    }
                };
            }
        }

        file_like.clone().as_socket()?.connect(endpoint).await?;
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
        let file_like = self.linux_process().get_file_like(sockfd.into())?;
        file_like
            .clone()
            .as_socket()?
            .setsockopt(level, optname, optval.as_slice(optlen)?)
    }

    /// get options for the socket referred to by the file descriptor sockfd.
    pub fn sys_getsockopt(
        &mut self,
        sockfd: usize,
        level: usize,
        optname: usize,
        optval: UserOutPtr<u32>,
        optlen: UserInOutPtr<u32>,
    ) -> SysResult {
        info!(
            "sys_getsockopt: sockfd:{}, level:{}, optname:{}, optval:{:?} , optlen:{:?}",
            sockfd, level, optname, optval, optlen
        );
        let level = match Level::try_from(level) {
            Ok(level) => level,
            Err(_) => {
                // Unknown levels (e.g. SOL_PACKET=263) — return a zeroed int.
                warn!("getsockopt: unsupported level: {}", level);
                return write_sockopt_out(optval, optlen, &0u32.to_ne_bytes());
            }
        };
        if optval.is_null() {
            return Err(LxError::EINVAL);
        }
        match level {
            Level::SOL_SOCKET => {
                // SO_PEERCRED (17): `struct ucred { pid_t pid; uid_t uid;
                // gid_t gid; }` — the credentials of the process on the other end
                // of a connected (unix) socket. seatd reads this to authorize a
                // Wayland client (labwc); without it the call returned ENOPROTOOPT
                // ("invalid optname: 17") and seatd refused the client. Eclipse is
                // single-user root, so report root uid/gid (which is what seatd
                // checks) and the peer's pid when the socket tracks it.
                const SO_PEERCRED: usize = 17;
                if optname == SO_PEERCRED {
                    let file_like = self.linux_process().get_file_like(sockfd.into())?;
                    let pid = file_like
                        .as_socket()
                        .ok()
                        .and_then(|s| s.peer_pid())
                        .unwrap_or(1);
                    let ucred: [u32; 3] = [pid as u32, 0, 0];
                    let mut bytes = [0u8; 12];
                    for (i, w) in ucred.iter().enumerate() {
                        bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_ne_bytes());
                    }
                    return write_sockopt_out(optval, optlen, &bytes);
                }
                let optname = match SolOptname::try_from(optname) {
                    Ok(optname) => optname,
                    Err(_) => {
                        error!("invalid optname: {}", optname);
                        return Err(LxError::ENOPROTOOPT);
                    }
                };

                let file_like = self.linux_process().get_file_like(sockfd.into())?;
                let (recv_buf_ca, send_buf_ca) = file_like
                    .clone()
                    .as_socket()?
                    .get_buffer_capacity()
                    .unwrap_or((64 * 1024, 64 * 1024));

                match optname {
                    SolOptname::SNDBUF => {
                        write_sockopt_out(optval, optlen, &(send_buf_ca as u32).to_ne_bytes())
                    }
                    SolOptname::RCVBUF => {
                        write_sockopt_out(optval, optlen, &(recv_buf_ca as u32).to_ne_bytes())
                    }
                    SolOptname::REUSEADDR => write_sockopt_out(optval, optlen, &1u32.to_ne_bytes()),
                    SolOptname::ERROR => {
                        let err = file_like.clone().as_socket()?.take_so_error();
                        write_sockopt_out(optval, optlen, &(err as u32).to_ne_bytes())
                    }
                    // struct linger { int l_onoff; int l_linger; } — zero-linger.
                    SolOptname::LINGER => write_sockopt_out(optval, optlen, &[0u8; 8]),
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
                    IpOptname::HDRINCL => write_sockopt_out(optval, optlen, &0u32.to_ne_bytes()),
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
            let endpoint =
                sockaddr_to_endpoint(read_sockaddr(dest_addr.as_addr(), addrlen)?, addrlen)?;
            info!("sys_sendto: sockfd:{:?}, endpoint:{:?}", sockfd, endpoint);
            Some(endpoint)
        };
        let file_like = self.linux_process().get_file_like(sockfd.into())?;
        // Return the socket's ACTUAL queued byte count, not the full requested
        // `len`. TCP `write()` can perform a short write (queues min(len, free TX
        // space)); reporting `len` regardless makes the caller believe bytes it
        // never sent were delivered, silently truncating the stream.
        let written = file_like
            .clone()
            .as_socket()?
            .write(buf.as_slice(len)?, endpoint)?;
        // Do not drain_net_poll here — busybox ping uses sendto; 32× poll_ifaces
        // blocks for a long time (smoltcp + SOCKETS lock). Sockets drive RX in read/poll.
        Ok(written)
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
        let _ = self.maybe_handle_tty_intr()?;
        linux_object::process::check_signals()?;
        info!(
            "sys_recvfrom: sockfd:{}, buffer:{:?}, length:{}, flags:{} , src_addr:{:?}, addrlen:{:?}",
            sockfd, buf, len, flags, src_addr, addrlen
        );
        let file_like = self.linux_process().get_file_like(sockfd.into())?;
        let old_flags = file_like.flags();
        let force_nonblock =
            (flags & MSG_DONTWAIT) != 0 && !old_flags.contains(OpenFlags::NON_BLOCK);
        if force_nonblock {
            file_like.set_flags(old_flags | OpenFlags::NON_BLOCK)?;
        }
        debug!("FileLike {} flags: {:?}", sockfd, file_like.flags());
        let cap_len = len.min(super::SYSCALL_IO_MAX);
        let mut data = vec![0u8; cap_len];
        let socket = file_like.as_socket()?;
        let (result, endpoint) = if (flags & MSG_PEEK) != 0 {
            socket.peek(&mut data).await
        } else {
            socket.read(&mut data).await
        };
        if force_nonblock {
            let _ = file_like.set_flags(old_flags);
        }
        if let Ok(received) = result {
            if !src_addr.is_null() {
                let sockaddr_in = SockAddr::from(endpoint);
                sockaddr_in.write_to(src_addr, addrlen)?;
            }
            buf.write_array(&data[..received])?;
            Ok(received)
        } else {
            result
        }
    }

    /// Parse `SCM_RIGHTS` ancillary data and resolve the carried fd numbers to
    /// the sender's open files, ready to be queued on the peer. `struct cmsghdr`
    /// is `{ size_t cmsg_len; int cmsg_level; int cmsg_type; }` (16 bytes on
    /// x86_64), followed by the fd array; entries are `CMSG_ALIGN`ed to 8.
    fn collect_scm_rights_fds(&self, ctrl: &[u8]) -> Vec<Arc<dyn FileLike>> {
        const CMSG_HDR_LEN: usize = 16;
        const SOL_SOCKET_LEVEL: i32 = 1;
        const SCM_RIGHTS: i32 = 1;
        let mut fds = Vec::new();
        let proc = self.linux_process();
        let mut off = 0usize;
        while off + CMSG_HDR_LEN <= ctrl.len() {
            let cmsg_len = u64::from_ne_bytes(ctrl[off..off + 8].try_into().unwrap()) as usize;
            let level = i32::from_ne_bytes(ctrl[off + 8..off + 12].try_into().unwrap());
            let typ = i32::from_ne_bytes(ctrl[off + 12..off + 16].try_into().unwrap());
            // `cmsg_len` is attacker-controlled; compute the message end with
            // checked arithmetic so a value near usize::MAX cannot wrap past the
            // `> ctrl.len()` guard (which would then slice with start > end and
            // panic the kernel).
            let cmsg_end = match off.checked_add(cmsg_len) {
                Some(end) if cmsg_len >= CMSG_HDR_LEN && end <= ctrl.len() => end,
                _ => break,
            };
            if level == SOL_SOCKET_LEVEL && typ == SCM_RIGHTS {
                for chunk in ctrl[off + CMSG_HDR_LEN..cmsg_end].chunks_exact(4) {
                    let raw = i32::from_ne_bytes(chunk.try_into().unwrap());
                    if raw >= 0 {
                        if let Ok(fl) = proc.get_file_like(FileDesc::from(raw as usize)) {
                            fds.push(fl);
                        }
                    }
                }
            }
            // CMSG_ALIGN(cmsg_len); use checked arithmetic and require forward
            // progress so a wrapped/zero step cannot spin forever.
            let step = match cmsg_len.checked_add(7).map(|v| v & !7) {
                Some(s) if s > 0 => s,
                _ => break,
            };
            off = match off.checked_add(step) {
                Some(n) => n,
                None => break,
            };
        }
        fds
    }

    /// transmit a message to another socket
    #[allow(unsafe_code)]
    pub fn sys_sendmsg(
        &mut self,
        sockfd: usize,
        msg: UserInPtr<MsgHdr>,
        _flags: usize,
    ) -> SysResult {
        info!(
            "sys_sendmsg: sockfd:{:?}, msg:{:?}, flags:{}",
            sockfd, msg, _flags
        );
        let hdr = msg.read()?;
        self.sendmsg_hdr(sockfd, &hdr)
    }

    /// Core of `sendmsg` for an already-read header (shared with `sendmmsg`).
    fn sendmsg_hdr(&mut self, sockfd: usize, hdr: &MsgHdr) -> SysResult {
        let iov_ptr: UserInPtr<IoVecIn> = hdr.msg_iov.as_addr().into();
        let iovlen = hdr.msg_iovlen;
        let iovs = iov_ptr.read_iovecs(iovlen)?;
        if iovs.total_len() > super::SYSCALL_IO_MAX {
            return Err(LxError::EINVAL);
        }
        let data = iovs.read_to_vec()?;

        // SCM_RIGHTS: resolve any attached fds before queueing the bytes.
        // `msg_controllen` is fully user-controlled; bound it before `read_array`
        // so a huge value cannot request a multi-GiB allocation (alloc abort) or
        // walk off the mapped control buffer. Linux bounds this by optmem_max.
        const CONTROL_MAX: usize = 64 * 1024;
        let passed_fds = if !hdr.msg_control.is_null() && hdr.msg_controllen >= 16 {
            if hdr.msg_controllen > CONTROL_MAX {
                return Err(LxError::EINVAL);
            }
            let ctrl = hdr.msg_control.read_array(hdr.msg_controllen)?;
            self.collect_scm_rights_fds(&ctrl)
        } else {
            Vec::new()
        };

        let endpoint = if !hdr.msg_name.is_null() {
            let namelen = hdr.msg_namelen as usize;
            let endpoint =
                sockaddr_to_endpoint(read_sockaddr(hdr.msg_name.as_addr(), namelen)?, namelen)?;
            Some(endpoint)
        } else {
            None
        };

        let file_like = self.linux_process().get_file_like(sockfd.into())?;
        let socket_fl = file_like.clone();
        let socket = socket_fl.as_socket()?;
        // Return the actual queued byte count (a TCP short write can queue less
        // than `data.len()`); reporting the full length silently drops the tail.
        // Queue the SCM_RIGHTS fds BEFORE the bytes so they are tagged with the
        // byte offset of the FIRST byte of this message. Linux delivers a passed
        // fd together with the recvmsg that returns the first accompanying data
        // byte; queueing after the write tagged the fds at the message's END, so
        // a peer that reads a header first and the body second (seatd/libseat)
        // got "Bad file descriptor" — the fd was withheld until the whole
        // message had been read.
        if !passed_fds.is_empty() {
            let _ = socket.send_fds(passed_fds);
        }
        let written = socket.write(&data, endpoint)?;
        Ok(written)
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
        let mut hdr = msg.read()?;
        // Capture the user address before `msg` may be moved by `write_to_msg`.
        let msg_addr = msg.as_addr();

        let iov_ptr = hdr.msg_iov;
        let iovlen = hdr.msg_iovlen;
        let mut iovs = iov_ptr.read_iovecs(iovlen)?;
        let total_len = iovs.total_len().min(super::SYSCALL_IO_MAX);
        let mut data = vec![0u8; total_len];

        let file_like = self.linux_process().get_file_like(sockfd.into())?;
        let old_flags = file_like.flags();
        let force_nonblock =
            (flags & MSG_DONTWAIT) != 0 && !old_flags.contains(OpenFlags::NON_BLOCK);
        if force_nonblock {
            file_like.set_flags(old_flags | OpenFlags::NON_BLOCK)?;
        }
        let socket = file_like.as_socket()?;
        let (result, endpoint) = if (flags & MSG_PEEK) != 0 {
            socket.peek(&mut data).await
        } else {
            socket.read(&mut data).await
        };
        if force_nonblock {
            let _ = file_like.set_flags(old_flags);
        }

        let addr = hdr.msg_name;
        if let Ok(len) = result {
            iovs.write_from_buf(&data[..len])?;
            if !addr.is_null() {
                let sockaddr_in = SockAddr::from(endpoint);
                sockaddr_in.write_to_msg(msg)?;
            }
            // SCM_RIGHTS: install any fds the peer attached and emit a cmsg.
            let mut ctrl_written = 0usize;
            if !hdr.msg_control.is_null() && hdr.msg_controllen >= 16 {
                let max_fds = (hdr.msg_controllen - 16) / 4;
                let fds = socket.recv_fds(max_fds);
                if !fds.is_empty() {
                    let cmsg_len = 16 + fds.len() * 4;
                    let mut cbuf = Vec::with_capacity(cmsg_len);
                    cbuf.extend_from_slice(&(cmsg_len as u64).to_ne_bytes());
                    cbuf.extend_from_slice(&1i32.to_ne_bytes()); // SOL_SOCKET
                    cbuf.extend_from_slice(&1i32.to_ne_bytes()); // SCM_RIGHTS
                    let proc = self.linux_process();
                    for fl in fds {
                        let newfd = proc.add_file(fl)?;
                        let raw: i32 = newfd.into();
                        cbuf.extend_from_slice(&raw.to_ne_bytes());
                    }
                    ctrl_written = cbuf.len().min(hdr.msg_controllen);
                    hdr.msg_control.write_array(&cbuf[..ctrl_written])?;
                }
            }
            // Linux ALWAYS reports how much ancillary data it wrote through
            // msg_controllen — 0 when there is none. Leaving the caller's IN
            // value (the buffer CAPACITY) there handed strict cmsg parsers a
            // buffer's worth of stale user memory to walk as if it were
            // kernel-written control headers: rustix (wayland-rs / lunarbg)
            // read a garbage cmsg_len there and panicked with
            // "range start index 18446744069414584321 out of range for slice
            // of length 136" — 136 being exactly the untouched capacity.
            // libwayland's C macros merely skidded over the same garbage.
            if !hdr.msg_control.is_null() {
                let controllen_addr = msg_addr + core::mem::offset_of!(MsgHdr, msg_controllen);
                let mut p = UserOutPtr::<usize>::from(controllen_addr);
                p.write(ctrl_written)?;
            }
            // Report truncation (and any other recv flags) via msg_flags.
            let msg_flags = socket.take_msg_flags();
            {
                let flags_addr = msg_addr + core::mem::offset_of!(MsgHdr, msg_flags);
                let mut p = UserOutPtr::<i32>::from(flags_addr);
                p.write(msg_flags)?;
            }
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
        let endpoint = sockaddr_to_endpoint(read_sockaddr(addr.as_addr(), addrlen)?, addrlen)?;
        debug!("sys_bind: fd:{} bind to {:?}", sockfd, endpoint);

        let proc = self.linux_process();
        if let Endpoint::Unix(path) = &endpoint {
            if !path.is_empty() {
                // Abstract-namespace sockets (leading NUL, e.g. X11's
                // `\0/tmp/.X11-unix/X0`) live only in the in-kernel registry and
                // have no filesystem node; only pathname sockets get a node.
                let is_abstract = path.starts_with('\0');
                if !is_abstract {
                    let (dir_path, file_name) = split_path(path);
                    match proc.lookup_inode_at(FileDesc::CWD, dir_path, true) {
                        Ok(dir_inode) => {
                            if dir_inode.find(file_name).is_err() {
                                if let Err(err) = dir_inode.create(
                                    file_name,
                                    linux_object::fs::vfs::FileType::Socket,
                                    0o666,
                                ) {
                                    warn!(
                                        "sys_bind: unable to create unix socket node {:?}: {:?}; continuing with in-kernel registration only",
                                        file_name, err
                                    );
                                } else {
                                    linux_object::fs::dcache_invalidate();
                                }
                            }
                        }
                        Err(err) => {
                            warn!(
                                "sys_bind: unable to lookup unix socket directory {:?}: {:?}; continuing with in-kernel registration only",
                                dir_path, err
                            );
                        }
                    }
                }

                let file_like = proc.get_file_like(sockfd.into())?;
                if let Ok(unix) = file_like.clone().downcast_arc::<UnixSocketState>() {
                    UnixSocketState::register(path.clone(), unix)?;
                }
            }
        }

        let file_like = proc.get_file_like(sockfd.into())?;
        file_like.clone().as_socket()?.bind(endpoint)
    }

    /// marks the socket referred to by sockfd as a passive socket,
    /// that is, as a socket that will be used to accept incoming connection
    pub fn sys_listen(&mut self, sockfd: usize, backlog: usize) -> SysResult {
        info!("sys_listen: fd:{}, backlog:{}", sockfd, backlog);
        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let file_like = self.linux_process().get_file_like(sockfd.into())?;
        file_like.clone().as_socket()?.listen()
    }

    /// shutdown a socket
    pub fn sys_shutdown(&mut self, sockfd: usize, howto: usize) -> SysResult {
        info!("sys_shutdown: sockfd:{}, howto:{}", sockfd, howto);
        let file_like = self.linux_process().get_file_like(sockfd.into())?;
        file_like.clone().as_socket()?.shutdown(howto)
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
        self.sys_accept4(sockfd, addr, addrlen, 0).await
    }

    /// Like [`Self::sys_accept`], but takes an extra `flags` argument that may
    /// carry `SOCK_NONBLOCK` (`0x800`) and/or `SOCK_CLOEXEC` (`0x80000`),
    /// applied to the newly accepted socket. These share the bit values of
    /// `O_NONBLOCK`/`O_CLOEXEC`, so they map directly onto [`OpenFlags`].
    pub async fn sys_accept4(
        &mut self,
        sockfd: usize,
        addr: UserOutPtr<SockAddr>,
        addrlen: UserInOutPtr<u32>,
        flags: usize,
    ) -> SysResult {
        info!(
            "sys_accept4: sockfd:{}, addr:{:?}, addrlen={:?}, flags={:#x}",
            sockfd, addr, addrlen, flags
        );
        // Validate flags BEFORE accept() consumes a connection from the queue.
        // SOCK_NONBLOCK / SOCK_CLOEXEC requested for the accepted socket; any
        // other bit is invalid (GLib's GDBus path only ever passes these two).
        // Previously this ran after accept(), so a bad flag accepted then
        // dropped an established client connection.
        const SOCK_NONBLOCK: usize = 0o4000;
        const SOCK_CLOEXEC: usize = 0o2000000;
        if flags & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
            return Err(LxError::EINVAL);
        }

        // smoltcp tcp sockets do not support backlog
        // open multiple sockets for each connection
        let file_like = self.linux_process().get_file_like(sockfd.into())?;
        let (new_socket, remote_endpoint) = file_like.clone().as_socket()?.accept().await?;
        debug!(
            "FileLike{} flags: {:?}, New flags: {:?}",
            sockfd,
            file_like.flags(),
            new_socket.flags()
        );

        if flags != 0 {
            let new_flags = OpenFlags::from_bits_truncate(flags);
            new_socket.set_flags(new_flags)?;
        }

        // Copy the peer address out BEFORE installing the fd, so a bad addr/
        // addrlen pointer fails the syscall (EFAULT) without leaking the
        // accepted fd into the process table (Linux copies the address out
        // before fd_install).
        if !addr.is_null() {
            let sockaddr_in = SockAddr::from(remote_endpoint);
            sockaddr_in.write_to(addr, addrlen)?;
        }
        let new_fd = self.linux_process().add_socket(new_socket)?;
        Ok(new_fd.into())
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
        let file_like = self.linux_process().get_file_like(sockfd.into())?;
        let endpoint = file_like
            .clone()
            .as_socket()?
            .endpoint()
            .ok_or(LxError::EINVAL)?;
        SockAddr::from(endpoint).write_to(addr, addrlen)?;
        Ok(0)
    }

    /// returns the address of the peer connected to the socket sockfd,
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
        let file_like = self.linux_process().get_file_like(sockfd.into())?;
        let remote_endpoint = file_like
            .clone()
            .as_socket()?
            .remote_endpoint()
            .ok_or(LxError::EINVAL)?;
        SockAddr::from(remote_endpoint).write_to(addr, addrlen)?;
        Ok(0)
    }

    /// creates a pair of connected sockets in the specified domain, of the specified type,
    /// and using the optionally specified protocol.
    pub fn sys_socketpair(
        &mut self,
        domain: usize,
        _type: usize,
        protocol: usize,
        mut sv: UserOutPtr<i32>,
    ) -> SysResult {
        info!(
            "sys_socketpair: domain:{}, type:{}, protocol:{}",
            domain, _type, protocol
        );
        if domain != Domain::AF_UNIX as usize {
            return Err(LxError::EAFNOSUPPORT);
        }
        let proc = self.linux_process();
        let socket1 = Arc::new(UnixSocketState::default());
        let socket2 = Arc::new(UnixSocketState::default());
        UnixSocketState::connect_pair(&socket1, &socket2);
        // The type argument packs SOCK_NONBLOCK / SOCK_CLOEXEC alongside the
        // socket type (same bit values as O_NONBLOCK / O_CLOEXEC, like
        // accept4). These were silently dropped, handing out BLOCKING sockets
        // to callers whose event loops assume nonblocking semantics —
        // Firefox's WaylandProxy (socketpair(AF_UNIX, SOCK_STREAM |
        // SOCK_NONBLOCK | SOCK_CLOEXEC)) drains with read-until-EAGAIN, so a
        // blocking pair wedged its forwarding thread and Wayland startup died
        // with "ProxiedConnection: broken source socket".
        const SOCK_NONBLOCK: usize = 0o4000;
        const SOCK_CLOEXEC: usize = 0o2000000;
        let flag_bits = _type & (SOCK_NONBLOCK | SOCK_CLOEXEC);
        if flag_bits != 0 {
            let new_flags = OpenFlags::from_bits_truncate(flag_bits);
            socket1.set_flags(new_flags)?;
            socket2.set_flags(new_flags)?;
        }
        let fd1 = proc.add_socket(socket1)?;
        let fd2 = proc.add_socket(socket2)?;
        sv.write_array(&[fd1.into(), fd2.into()])?;
        Ok(0)
    }

    /// `sendmmsg`: send an array of messages in one call. glibc's resolver
    /// fires its parallel A+AAAA DNS queries through this; without it every
    /// Firefox name lookup spammed `unknown syscall: SENDMMSG` and fell back.
    /// Sequential delegation to the sendmsg core; each entry's `msg_len` is
    /// written back. On error: fail if nothing was sent, else report the count.
    pub fn sys_sendmmsg(
        &mut self,
        sockfd: usize,
        msgvec: UserInOutPtr<u8>,
        vlen: usize,
        _flags: usize,
    ) -> SysResult {
        info!(
            "sys_sendmmsg: sockfd:{}, msgvec:{:?}, vlen:{}",
            sockfd, msgvec, vlen
        );
        const MMSG_MAX: usize = 64;
        let base = msgvec.as_addr();
        let stride = core::mem::size_of::<MsgHdr>() + 8; // + u32 msg_len + pad
        let mut sent = 0usize;
        for i in 0..vlen.min(MMSG_MAX) {
            let hdr_ptr: UserInPtr<MsgHdr> = (base + i * stride).into();
            let hdr = hdr_ptr.read()?;
            match self.sendmsg_hdr(sockfd, &hdr) {
                Ok(n) => {
                    let mut len_ptr: UserOutPtr<u32> =
                        (base + i * stride + core::mem::size_of::<MsgHdr>()).into();
                    len_ptr.write(n as u32)?;
                    sent += 1;
                }
                Err(e) => {
                    if sent == 0 {
                        return Err(e);
                    }
                    break;
                }
            }
        }
        Ok(sent)
    }

    /// `recvmmsg`: receive up to `vlen` messages. The first receive honors the
    /// caller's blocking mode; subsequent ones are forced non-blocking so the
    /// call returns as soon as the queue drains (Linux semantics without the
    /// timeout extra, which the resolver does not rely on).
    pub async fn sys_recvmmsg(
        &mut self,
        sockfd: usize,
        msgvec: UserInOutPtr<u8>,
        vlen: usize,
        flags: usize,
    ) -> SysResult {
        info!(
            "sys_recvmmsg: sockfd:{}, msgvec:{:?}, vlen:{}",
            sockfd, msgvec, vlen
        );
        const MMSG_MAX: usize = 64;
        let base = msgvec.as_addr();
        let stride = core::mem::size_of::<MsgHdr>() + 8;
        let mut received = 0usize;
        for i in 0..vlen.min(MMSG_MAX) {
            let hdr_ptr: UserInOutPtr<MsgHdr> = (base + i * stride).into();
            let per_flags = if i == 0 { flags } else { flags | MSG_DONTWAIT };
            match self.sys_recvmsg(sockfd, hdr_ptr, per_flags).await {
                Ok(n) => {
                    let mut len_ptr: UserOutPtr<u32> =
                        (base + i * stride + core::mem::size_of::<MsgHdr>()).into();
                    len_ptr.write(n as u32)?;
                    received += 1;
                }
                Err(LxError::EAGAIN) if received > 0 => break,
                Err(e) => {
                    if received == 0 {
                        return Err(e);
                    }
                    break;
                }
            }
        }
        Ok(received)
    }

    /// Eclipse-specific DNS/hosts lookup for userland shims (`libeclipse_dns.so`).
    pub fn sys_eclipse_dns_query(
        &self,
        name: UserInPtr<u8>,
        name_len: usize,
        family: usize,
        out: UserOutPtr<linux_object::net::dns::DnsResultEntry>,
        out_max: usize,
    ) -> SysResult {
        use linux_object::fs::dns_vfs_root;
        use linux_object::net::dns::{self, DnsFamily};

        if out_max == 0 {
            return Ok(0);
        }
        if name_len == 0 || name_len > 253 {
            return Err(LxError::EINVAL);
        }
        let hostname = name.as_str(name_len).map_err(|_| LxError::EINVAL)?;
        let root_inode = dns_vfs_root().ok_or(LxError::ENOENT)?;
        let addrs = dns::resolve(&root_inode, hostname, DnsFamily::from_usize(family))?;
        let n = addrs.len().min(out_max);
        for (i, ip) in addrs.iter().take(n).enumerate() {
            out.add(i)
                .write(linux_object::net::dns::DnsResultEntry::from_ip(*ip))?;
        }
        Ok(n as _)
    }
}

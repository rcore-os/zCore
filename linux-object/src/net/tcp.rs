// Tcpsocket

// crate
use crate::error::{LxError, LxResult};
use crate::fs::{FileLike, OpenFlags, PollStatus};
use crate::net::*;
use alloc::sync::Arc;
use kernel_hal::thread;
use lock::Mutex;

// alloc
use alloc::boxed::Box;
use alloc::vec;

// smoltcp
use smoltcp::socket::{TcpSocket, TcpSocketBuffer, TcpState};
use smoltcp::wire::{IpAddress, Ipv4Address, Ipv6Address};

// async
use async_trait::async_trait;

// third part
#[allow(unused_imports)]
use zircon_object::object::*;

/// TCP socket structure
pub struct TcpSocketState {
    /// Kernel object base
    base: KObjectBase,
    /// TcpSocket Inner
    inner: Arc<Mutex<TcpInner>>,
}

/// TCP socket inner
#[derive(Debug)]
pub struct TcpInner {
    /// missing documentation
    handle: GlobalSocketHandle,
    /// missing documentation
    local_endpoint: Option<IpEndpoint>, // save local endpoint for bind()
    /// missing documentation
    is_listening: bool,
    /// flags on the socket
    flags: OpenFlags,
    /// ipv6 domain socket flag
    ipv6: bool,
}

impl Drop for TcpInner {
    fn drop(&mut self) {
        // A listening socket reserves its port in LISTEN_TABLE. `shutdown()`
        // clears it, but a plain `close()`/process-exit just drops this object;
        // without this Drop the port would leak and stay permanently EADDRINUSE
        // (the classic "restart the server after a crash" failure). TcpInner is
        // the shared Arc payload, so this runs only once the last reference
        // (including any dup()'d fds) is gone. `unlisten` is idempotent, so it is
        // harmless if `shutdown()` already cleared the entry.
        if self.is_listening {
            if let Some(ep) = self.local_endpoint {
                crate::net::LISTEN_TABLE.unlisten(ep.port);
            }
        }
    }
}

impl TcpSocketState {
    /// missing documentation
    pub fn new(ipv6: bool) -> LxResult<Self> {
        let rx_buffer = TcpSocketBuffer::new(vec![0; TCP_RECVBUF]);
        let tx_buffer = TcpSocketBuffer::new(vec![0; TCP_SENDBUF]);
        let socket = TcpSocket::new(rx_buffer, tx_buffer);
        let handle = super::register_smoltcp_socket(socket)?;

        Ok(TcpSocketState {
            base: KObjectBase::new(),
            inner: Arc::new(Mutex::new(TcpInner {
                handle,
                local_endpoint: None,
                is_listening: false,
                flags: OpenFlags::RDWR,
                ipv6,
            })),
        })
    }

    fn endpoint_matches_family(ipv6: bool, ep: &IpEndpoint) -> bool {
        matches!(
            (ipv6, ep.addr),
            (true, IpAddress::Ipv6(_)) | (false, IpAddress::Ipv4(_))
        )
    }
}

#[async_trait]
impl Socket for TcpSocketState {
    /// read to buffer
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        let (handle, flags) = {
            let inner = self.inner.lock();
            (inner.handle.0, inner.flags)
        };
        debug!(
            "tcp read handle={} req_len={} nonblock={}",
            handle,
            data.len(),
            flags.contains(OpenFlags::NON_BLOCK)
        );
        let deadline = kernel_hal::timer::timer_now() + core::time::Duration::from_secs(120);
        loop {
            // Drive the NIC FIRST so any deferred RX is in the socket before
            // recv_slice is called. Use the UNTHROTTLED drain here: this is a
            // blocking read actively waiting for data, so we must pull RX as
            // fast as it arrives. The throttled tick (every 4–32 ms) lets a
            // fast download overflow the e1000e RX ring (~384 KiB fills in a
            // few ms on a real link) before we drain it — packets drop and the
            // large transfer wedges, while small ones that fit the ring work.
            // The aggressive poll stops the instant recv_slice returns data.
            kernel_hal::deferred_job::drain_deferred_jobs();
            crate::net::drain_net_urgent();

            let sets = get_sockets();
            let mut sets = sets.lock();
            let mut socket = sets.get::<TcpSocket>(handle);

            let state = socket.state();

            let mut copied_len = socket.recv_slice(data);
            if let Ok(0) = copied_len {
                if !data.is_empty() {
                    copied_len = Err(smoltcp::Error::Exhausted);
                }
            }

            // Receive-half EOF must be decided by smoltcp's `may_recv()`, NOT by
            // the raw TCP state. `may_recv()` stays true while the peer can still
            // send — ESTABLISHED, and crucially FIN-WAIT-1/FIN-WAIT-2, where WE
            // closed our transmit half but the peer keeps streaming (exactly what
            // an HTTP client does: it `shutdown(SHUT_WR)`s after sending the
            // request, then reads the response body). It only goes false once the
            // peer's FIN has been received AND the receive buffer is drained
            // (CLOSE-WAIT/CLOSING/LAST-ACK/TIME-WAIT/CLOSED with no buffered data).
            //
            // The previous code treated FIN-WAIT-2 as "peer closed" and returned
            // EOF the moment `recv_slice` was momentarily Exhausted mid-transfer,
            // which truncated downloads intermittently (apk then RSA-verified a
            // short APKINDEX -> "BAD signature").
            let recv_closed = !socket.may_recv();
            trace!(
                "[tcp read] state={:?} recv_closed={} result={:?}",
                state,
                recv_closed,
                copied_len
            );
            drop(socket);
            drop(sets);

            // Receive half closed and nothing left to read -> real EOF.
            if recv_closed {
                if let Err(smoltcp::Error::Exhausted) = copied_len {
                    // Log the terminal state at error! (survives LOG=error) so a
                    // truncated download (peer FIN/RST mid-transfer -> apk gets a
                    // short APKINDEX) is visible in dmesg: state names WHY the
                    // receive half closed (a clean CLOSE-WAIT after all data, vs
                    // a RST/abort). Fires at most once per connection close.
                    error!(
                        "[tcp read] EOF handle={} state={:?} may_recv=false (recv half closed)",
                        handle, state
                    );
                    return (Ok(0), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                }
            }

            match copied_len {
                Err(smoltcp::Error::Exhausted) => {
                    if flags.contains(OpenFlags::NON_BLOCK) {
                        return (Err(LxError::EAGAIN), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                    }
                    // Hard timeout: avoid blocking forever if the peer goes
                    // silent. Return ETIMEDOUT, NOT Ok(0): a length-0 read is
                    // an orderly-shutdown (EOF) signal, so faking it on a still-
                    // open connection makes the caller believe the peer closed
                    // cleanly and treat a truncated transfer as complete (this
                    // is exactly the "short APKINDEX -> BAD signature" class of
                    // corruption). An error lets the caller retry/fail correctly.
                    if kernel_hal::timer::timer_now() >= deadline {
                        let (may, rq, sq) = {
                            let s = get_sockets();
                            let mut s = s.lock();
                            let sk = s.get::<TcpSocket>(handle);
                            (sk.may_recv(), sk.recv_queue(), sk.send_queue())
                        };
                        error!(
                            "[tcp read] deadline exceeded -> EOF handle={} state={:?} may_recv={} recv_queue={} send_queue={} ({})",
                            handle, state, may, rq, sq,
                            if rq > 65536 {
                                "CONSUMER-BLOCKED (app not reading / downstream stall)"
                            } else if rq == 0 && may {
                                "PEER-SILENT (window open, no data: lost ACK/window-update)"
                            } else {
                                "OTHER"
                            }
                        );
                        return (Ok(0), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                    }
                }
                Ok(size) => {
                    crate::net::drain_net_urgent();
                    let endpoint = get_sockets()
                        .lock()
                        .get::<TcpSocket>(handle)
                        .remote_endpoint();
                    return (Ok(size), Endpoint::Ip(endpoint));
                }
                Err(smoltcp::Error::Finished) => {
                    return (Ok(0), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                }
                Err(err) => {
                    error!("Tcp socket read error: {:?}", err);
                    return (
                        Err(LxError::ENOTCONN),
                        Endpoint::Ip(IpEndpoint::UNSPECIFIED),
                    );
                }
            }
            if let Err(e) = crate::process::check_and_deliver_tty_interrupt() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            // Also honor non-TTY signals (SIGTERM/SIGALRM/...) so a blocked
            // recv returns EINTR, matching the udp/raw/icmp socket paths.
            if let Err(e) = crate::process::check_signals() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            // Park until the NIC's RX IRQ wakes us (immediate on data) or a
            // short fallback timer fires, instead of busy-spinning with
            // yield_now — which pegged a core at 100% for any socket blocked in
            // recv (e.g. an idle irssi). The 5 ms fallback still drives
            // poll_ifaces if a wake is ever missed, so a stalled wake can never
            // freeze the op the way a pure timer/IRQ park once did.
            kernel_hal::net::NetRxOrTimeoutFuture::new(5).await;
        }
    }
    async fn peek(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        let (handle, flags) = {
            let inner = self.inner.lock();
            (inner.handle.0, inner.flags)
        };
        loop {
            kernel_hal::deferred_job::drain_deferred_jobs();
            crate::net::drain_net_tick();

            let sets = get_sockets();
            let mut sets = sets.lock();
            let mut socket = sets.get::<TcpSocket>(handle);
            let mut copied_len = socket.peek_slice(data);
            if let Ok(0) = copied_len {
                if !data.is_empty() {
                    copied_len = Err(smoltcp::Error::Exhausted);
                }
            }
            drop(socket);
            drop(sets);
            match copied_len {
                Err(smoltcp::Error::Exhausted) => {
                    if flags.contains(OpenFlags::NON_BLOCK) {
                        return (Err(LxError::EAGAIN), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                    }
                }
                Ok(size) => {
                    let endpoint = get_sockets()
                        .lock()
                        .get::<TcpSocket>(handle)
                        .remote_endpoint();
                    return (Ok(size), Endpoint::Ip(endpoint));
                }
                Err(smoltcp::Error::Finished) => {
                    return (Ok(0), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
                }
                Err(err) => {
                    error!("Tcp socket peek error: {:?}", err);
                    return (
                        Err(LxError::ENOTCONN),
                        Endpoint::Ip(IpEndpoint::UNSPECIFIED),
                    );
                }
            }
            if let Err(e) = crate::process::check_and_deliver_tty_interrupt() {
                return (Err(e), Endpoint::Ip(IpEndpoint::UNSPECIFIED));
            }
            // Park on the RX IRQ waker with a 5 ms fallback — see read().
            kernel_hal::net::NetRxOrTimeoutFuture::new(5).await;
        }
    }
    /// write from buffer
    fn write(&self, data: &[u8], _sendto_endpoint: Option<Endpoint>) -> SysResult {
        let (handle, flags) = {
            let inner = self.inner.lock();
            (inner.handle.0, inner.flags)
        };
        if data.is_empty() {
            return Ok(0);
        }
        // Retry until at least one byte is queued. A full TX buffer returns
        // Ok(0); for a blocking socket we must keep draining ACKs (poll_ifaces)
        // and try again instead of returning a 0-length write, which makes
        // libc/busybox spin or treat the write as failed.
        let deadline = kernel_hal::timer::timer_now() + core::time::Duration::from_secs(30);
        loop {
            let copied_len = {
                let sets = get_sockets();
                let mut sets = sets.lock();
                let mut socket = sets.get::<TcpSocket>(handle);
                socket.send_slice(data)
            };
            crate::net::drain_net_tick();

            match copied_len {
                Ok(0) => {
                    if flags.contains(OpenFlags::NON_BLOCK) {
                        return Err(LxError::EAGAIN);
                    }
                    if kernel_hal::timer::timer_now() >= deadline {
                        warn!("[tcp write] TX buffer full, deadline exceeded");
                        return Err(LxError::ENOBUFS);
                    }
                    // Synchronous trait: drain ACKs so the peer's window frees
                    // up TX buffer space before retrying.
                    kernel_hal::deferred_job::drain_deferred_jobs();
                    crate::net::drain_net_urgent();
                }
                Ok(size) => {
                    flush_socket_egress();
                    return Ok(size);
                }
                Err(err) => {
                    // smoltcp returns `Illegal` once the socket can no longer
                    // send: it has left Established/CloseWait, i.e. the peer
                    // reset or closed the connection mid-stream. The correct
                    // errno for a write on a torn-down connection is EPIPE
                    // ("broken pipe") — not ENOBUFS, whose "No buffer space
                    // available" text made TLS libraries report a bogus
                    // "handshake failed: No buffer space available".
                    warn!(
                        "[tcp write] send failed: {:?} (connection no longer sendable)",
                        err
                    );
                    return Err(LxError::EPIPE);
                }
            }
        }
    }
    /// connect
    async fn connect(&self, endpoint: Endpoint) -> SysResult {
        let (handle, ipv6, non_block) = {
            let inner = self.inner.lock();
            (
                inner.handle.0,
                inner.ipv6,
                inner.flags.contains(OpenFlags::NON_BLOCK),
            )
        };
        let Endpoint::Ip(ip) = endpoint else {
            error!("connect: bad endpoint");
            return Err(LxError::EINVAL);
        };
        if !Self::endpoint_matches_family(ipv6, &ip) {
            return Err(LxError::EINVAL);
        }

        {
            let sockets = get_sockets();
            let mut sets = sockets.lock();
            let socket = sets.get::<TcpSocket>(handle);
            // A connect() while the handshake is still in progress must return
            // EALREADY (POSIX), not EISCONN. Apps re-issue connect() to poll a
            // non-blocking connect to completion; EISCONN makes them believe the
            // socket is connected and write() on SynSent -> EPIPE.
            if matches!(socket.state(), TcpState::SynSent | TcpState::SynReceived) {
                return Err(LxError::EALREADY);
            }
            if socket.is_active() {
                return Err(LxError::EISCONN);
            }
        }

        get_sockets()
            .lock()
            .get::<TcpSocket>(handle)
            .connect(ip, get_ephemeral_port())
            .map_err(|_| LxError::ENOBUFS)?;

        prepare_ipv4_stack();
        drain_net_poll(8);

        let state = get_sockets().lock().get::<TcpSocket>(handle).state();
        if matches!(state, TcpState::Established) {
            flush_socket_egress();
            return Ok(0);
        }
        if non_block {
            if matches!(state, TcpState::SynSent | TcpState::SynReceived) {
                return Err(LxError::EINPROGRESS);
            }
            if matches!(state, TcpState::Closed | TcpState::TimeWait) {
                return Err(LxError::ECONNREFUSED);
            }
        }

        let deadline = kernel_hal::timer::timer_now() + core::time::Duration::from_secs(30);
        loop {
            drain_net_poll(4);
            kernel_hal::deferred_job::drain_deferred_jobs();

            match get_sockets().lock().get::<TcpSocket>(handle).state() {
                TcpState::SynSent | TcpState::SynReceived => {}
                TcpState::Established => {
                    flush_socket_egress();
                    return Ok(0);
                }
                TcpState::Closed | TcpState::TimeWait => {
                    // The peer refused the connection (RST moves SynSent->Closed).
                    // Report it immediately with the correct errno instead of
                    // spinning until the 30s deadline and returning ETIMEDOUT.
                    // Mirrors the non-blocking path above. ETIMEDOUT is still
                    // returned below for the SynSent/SynReceived no-response case.
                    return Err(LxError::ECONNREFUSED);
                }
                other => {
                    warn!("connect: unexpected state {:?}, retrying", other);
                }
            }

            if kernel_hal::timer::timer_now() >= deadline {
                warn!("connect: timed out after 30s");
                return Err(LxError::ETIMEDOUT);
            }

            // Park on the RX IRQ waker (5 ms fallback) while the handshake
            // completes, rather than busy-spinning — see read().
            kernel_hal::net::NetRxOrTimeoutFuture::new(5).await;
        }
    }
    /// wait for some event on a file descriptor
    fn poll(&self, events: PollEvents) -> (bool, bool, bool) {
        //poll_ifaces();
        let inner = self.inner.lock();
        let (recv_state, send_state) = {
            let sets = get_sockets();
            let mut sets = sets.lock();
            let socket = sets.get::<TcpSocket>(inner.handle.0);
            debug!(
                "tcp is_listening: {:?}, now tcp state: {:?}",
                inner.is_listening,
                socket.state()
            );

            (socket.can_recv(), socket.can_send())
        };
        if (events.contains(PollEvents::IN) && !recv_state)
            || (events.contains(PollEvents::OUT) && !send_state)
        {
            crate::net::drain_net_tick();
        }

        let (mut read, mut write, mut error) = (false, false, false);

        let sets = get_sockets();
        let mut sets = sets.lock();
        let socket = sets.get::<TcpSocket>(inner.handle.0);

        if inner.is_listening {
            // A pending connection may already be in CloseWait (peer closed right
            // after connecting); signal POLLIN so accept() runs instead of the
            // listener silently wedging. Mirrors the accept() state check.
            read = matches!(socket.state(), TcpState::Established | TcpState::CloseWait);
        } else if !socket.is_open() {
            error = true;
            read = true;
            write = true;
        } else {
            if socket.can_recv() {
                read = true; // POLLIN
            } else {
                match socket.state() {
                    TcpState::CloseWait
                    | TcpState::Closing
                    | TcpState::LastAck
                    | TcpState::TimeWait => {
                        read = true;
                    }
                    _ => {}
                }
            }
            if socket.can_send() {
                write = true; // POLLOUT
            }
        }
        debug!("tcp poll: {:?}", (read, write, error));
        (read, write, error)
    }

    fn bind(&self, endpoint: Endpoint) -> SysResult {
        let mut inner = self.inner.lock();
        if let Endpoint::Ip(mut ip) = endpoint {
            if !Self::endpoint_matches_family(inner.ipv6, &ip) {
                return Err(LxError::EINVAL);
            }
            if ip.port == 0 {
                ip.port = get_ephemeral_port();
            }
            inner.local_endpoint = Some(ip);
            inner.is_listening = false;
            Ok(0)
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn listen(&self) -> SysResult {
        let mut inner = self.inner.lock();
        if inner.is_listening {
            info!("It's already listening");
            return Ok(0);
        }

        let local_endpoint = inner.local_endpoint.ok_or(LxError::EINVAL)?;
        info!("socket listening on {:?}", local_endpoint);

        if !crate::net::LISTEN_TABLE.can_listen(local_endpoint.port) {
            return Err(LxError::EADDRINUSE);
        }

        get_sockets()
            .lock()
            .get::<TcpSocket>(inner.handle.0)
            .listen(local_endpoint)
            .map_err(|_| LxError::ENOBUFS)?;

        crate::net::LISTEN_TABLE.listen(local_endpoint)?;
        inner.is_listening = true;
        Ok(0)
    }

    fn shutdown(&self, howto: usize) -> SysResult {
        let mut inner = self.inner.lock();
        if inner.is_listening {
            if let Some(ep) = inner.local_endpoint {
                crate::net::LISTEN_TABLE.unlisten(ep.port);
            }
            inner.is_listening = false;
        }
        let sets = get_sockets();
        let mut sets = sets.lock();
        let mut socket = sets.get::<TcpSocket>(inner.handle.0);
        if howto == 1 || howto == 2 {
            socket.close();
        }
        Ok(0)
    }

    async fn accept(&self) -> LxResult<(Arc<dyn FileLike>, Endpoint)> {
        let (endpoint, non_block, is_ipv6) = {
            let inner = self.inner.lock();
            (
                inner.local_endpoint.ok_or(LxError::EINVAL)?,
                inner.flags.contains(OpenFlags::NON_BLOCK),
                inner.ipv6,
            )
        };

        loop {
            crate::net::drain_net_tick();
            kernel_hal::deferred_job::drain_deferred_jobs();

            let established = {
                let handle = self.inner.lock().handle.0;
                let sockets = get_sockets();
                let mut sockets = sockets.lock();
                let socket = sockets.get::<TcpSocket>(handle);
                // Accept any post-handshake connection, not only `Established`.
                // A peer that connects and immediately closes (health checks,
                // port scans) drives the listen socket Established->CloseWait
                // within a single `poll_ifaces()` batch, so the transient
                // `Established` is often never observed. If we only matched
                // `Established`, the listener swap below would never run: the
                // socket stays stuck in CloseWait (with a remote endpoint set,
                // so smoltcp rejects new SYNs) and the port goes permanently
                // deaf. The child accepted in CloseWait correctly delivers any
                // buffered data and then EOF.
                matches!(socket.state(), TcpState::Established | TcpState::CloseWait)
            };

            if established {
                let listen_handle = self.inner.lock().handle.0;
                let (local, remote) = {
                    let sockets = get_sockets();
                    let mut sockets = sockets.lock();
                    let socket = sockets.get::<TcpSocket>(listen_handle);
                    (socket.local_endpoint(), socket.remote_endpoint())
                };

                let rx_buffer = TcpSocketBuffer::new(super::kernel_vec_zeroed(super::TCP_RECVBUF)?);
                let tx_buffer = TcpSocketBuffer::new(super::kernel_vec_zeroed(super::TCP_SENDBUF)?);
                let mut new_listen = TcpSocket::new(rx_buffer, tx_buffer);
                new_listen.listen(endpoint).map_err(|_| LxError::ENOBUFS)?;

                let new_listen_handle = {
                    let sockets = get_sockets();
                    let mut sockets = sockets.lock();
                    // Respect the global socket-count cap that bounds the fixed
                    // kernel heap (each socket pins SEND/RECV buffers). accept()
                    // added directly via SocketSet::add and so bypassed the cap
                    // that register_smoltcp_socket() enforces for every other
                    // socket creation.
                    if super::smoltcp_socket_count(&sockets) >= super::MAX_SMOLTCIP_SOCKETS {
                        return Err(LxError::ENOBUFS);
                    }
                    sockets.add(new_listen)
                };

                let child_handle = {
                    let mut inner = self.inner.lock();
                    core::mem::replace(&mut inner.handle, GlobalSocketHandle(new_listen_handle))
                };

                let new_socket = Arc::new(TcpSocketState {
                    base: KObjectBase::new(),
                    inner: Arc::new(Mutex::new(TcpInner {
                        handle: child_handle,
                        local_endpoint: Some(local),
                        is_listening: false,
                        flags: OpenFlags::RDWR,
                        ipv6: is_ipv6,
                    })),
                });
                return Ok((new_socket as Arc<dyn FileLike>, Endpoint::Ip(remote)));
            } else {
                if non_block {
                    return Err(LxError::EAGAIN);
                }
                thread::sleep_until(
                    kernel_hal::timer::timer_now() + core::time::Duration::from_millis(5),
                )
                .await;
            }
        }
    }

    fn endpoint(&self) -> Option<Endpoint> {
        let inner = self.inner.lock();
        let ep = inner.local_endpoint.unwrap_or_else(|| {
            let sets = get_sockets();
            let mut sets = sets.lock();
            let socket = sets.get::<TcpSocket>(inner.handle.0);
            socket.local_endpoint()
        });
        let addr = if ep.addr.is_unspecified() {
            if inner.ipv6 {
                IpAddress::Ipv6(Ipv6Address::UNSPECIFIED)
            } else {
                IpAddress::Ipv4(Ipv4Address::UNSPECIFIED)
            }
        } else {
            ep.addr
        };
        Some(Endpoint::Ip(IpEndpoint::new(addr, ep.port)))
    }

    fn remote_endpoint(&self) -> Option<Endpoint> {
        // Copy what we need out of `inner` and DROP its guard before locking the
        // global socket set. Every hot path (poll/endpoint/read/connect) takes
        // inner->SOCKETS; taking SOCKETS->inner here is a lock-order inversion
        // that deadlocks two threads sharing one fd (these are spinlocks).
        let (handle, ipv6) = {
            let inner = self.inner.lock();
            (inner.handle.0, inner.ipv6)
        };
        let sets = get_sockets();
        let mut sets = sets.lock();
        let socket = sets.get::<TcpSocket>(handle);
        if socket.is_open() {
            let ep = socket.remote_endpoint();
            let addr = if ep.addr.is_unspecified() {
                if ipv6 {
                    IpAddress::Ipv6(Ipv6Address::UNSPECIFIED)
                } else {
                    IpAddress::Ipv4(Ipv4Address::UNSPECIFIED)
                }
            } else {
                ep.addr
            };
            Some(Endpoint::Ip(IpEndpoint::new(addr, ep.port)))
        } else {
            None
        }
    }

    fn get_buffer_capacity(&self) -> Option<(usize, usize)> {
        // Read the handle and drop the `inner` guard before locking SOCKETS to
        // preserve the inner->SOCKETS order (avoids the ABBA deadlock).
        let handle = self.inner.lock().handle.0;
        let sockets = get_sockets();
        let mut set = sockets.lock();
        let socket = set.get::<TcpSocket>(handle);
        let (recv_ca, send_ca) = (socket.recv_capacity(), socket.send_capacity());
        Some((recv_ca, send_ca))
    }

    fn socket_type(&self) -> Option<SocketType> {
        Some(SocketType::SOCK_STREAM)
    }
}

impl_kobject!(TcpSocketState);

#[async_trait]
impl FileLike for TcpSocketState {
    fn flags(&self) -> OpenFlags {
        self.inner.lock().flags
    }

    fn set_flags(&self, f: OpenFlags) -> LxResult {
        let flags = &mut self.inner.lock().flags;

        // See fcntl, only O_APPEND, O_ASYNC, O_DIRECT, O_NOATIME, O_NONBLOCK
        flags.set(OpenFlags::APPEND, f.contains(OpenFlags::APPEND));
        flags.set(OpenFlags::NON_BLOCK, f.contains(OpenFlags::NON_BLOCK));
        flags.set(OpenFlags::CLOEXEC, f.contains(OpenFlags::CLOEXEC));
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            inner: self.inner.clone(),
        })
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        Socket::read(self, buf).await.0
    }

    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
        // Sockets do not support positioned reads.
        Err(LxError::ESPIPE)
    }

    fn write(&self, buf: &[u8]) -> LxResult<usize> {
        Socket::write(self, buf, None)
    }

    fn poll(&self, events: PollEvents) -> LxResult<PollStatus> {
        let (read, write, error) = Socket::poll(self, events);
        Ok(PollStatus { read, write, error })
    }

    async fn async_poll(&self, events: PollEvents) -> LxResult<PollStatus> {
        let (mut read, mut write, mut error) = Socket::poll(self, events);
        let ready = (events.contains(PollEvents::IN) && read)
            || (events.contains(PollEvents::OUT) && write)
            || error;
        if !ready {
            kernel_hal::net::NetRxOrTimeoutFuture::new(5).await;
            (read, write, error) = Socket::poll(self, events);
        }
        Ok(PollStatus { read, write, error })
    }

    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize> {
        let ipv6 = self.inner.lock().ipv6;
        handle_net_ioctl(request, arg1, arg2, arg3, ipv6)
    }

    fn as_socket(&self) -> LxResult<&dyn Socket> {
        Ok(self)
    }
}
#[cfg(test)]
mod transfer_bench {
    //! Host bench: a large TCP transfer over a smoltcp loopback interface using
    //! the *real* socket buffer sizes (TCP_RECVBUF/TCP_SENDBUF), with a
    //! deliberately slow reader so the receive buffer fills and the connection
    //! must ride the window / zero-window update path — exactly what separates a
    //! large download from a small one. A stall detector fails the test if no
    //! progress is made for a long time (a window-update deadlock), and the
    //! payload is verified byte-for-byte (corruption).
    //!
    //! QEMU x86_64 runs with `-nic none`, so this is the only place we can
    //! reproduce a large transfer off real hardware. If it passes, large-
    //! transfer flow-control with our config is sound and the bug is in the
    //! driver/integration; if it stalls or corrupts, we found it here.

    use alloc::collections::BTreeMap;
    use alloc::vec;
    use smoltcp::iface::{InterfaceBuilder, Routes};
    use smoltcp::phy::{Loopback, Medium};
    use smoltcp::socket::{SocketSet, TcpSocket, TcpSocketBuffer};
    use smoltcp::time::Instant;
    use smoltcp::wire::{IpAddress, IpCidr};

    fn mk_sock() -> TcpSocket<'static> {
        TcpSocket::new(
            TcpSocketBuffer::new(vec![0u8; crate::net::TCP_RECVBUF]),
            TcpSocketBuffer::new(vec![0u8; crate::net::TCP_SENDBUF]),
        )
    }

    fn run_transfer(total: usize, reader_chunk: usize) {
        let device = Loopback::new(Medium::Ip);
        let mut iface = InterfaceBuilder::new(device)
            .ip_addrs([IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8)])
            .routes(Routes::new(BTreeMap::new()))
            .finalize();

        let mut sockets = SocketSet::new(vec![]);
        let sh = sockets.add(mk_sock());
        let ch = sockets.add(mk_sock());

        sockets.get::<TcpSocket>(sh).listen(1234).unwrap();
        sockets
            .get::<TcpSocket>(ch)
            .connect((IpAddress::v4(127, 0, 0, 1), 1234), 49152)
            .unwrap();

        let mut sent = 0usize;
        let mut recvd = 0usize;
        let mut rbuf = vec![0u8; reader_chunk];
        let mut clock = 0i64;
        let mut idle = 0u64;

        while recvd < total {
            clock += 1; // advance 1 ms per poll so smoltcp timers progress
            let _ = iface.poll(&mut sockets, Instant::from_millis(clock));

            // Sender: push as much as the send window allows.
            {
                let mut s = sockets.get::<TcpSocket>(sh);
                while sent < total && s.can_send() {
                    let remaining = total - sent;
                    let mut chunk = vec![0u8; remaining.min(32 * 1024)];
                    for (j, b) in chunk.iter_mut().enumerate() {
                        *b = (sent + j) as u8;
                    }
                    match s.send_slice(&chunk) {
                        Ok(n) if n > 0 => sent += n,
                        _ => break,
                    }
                }
            }

            // Receiver: read at most `reader_chunk` per poll — the throttle that
            // keeps the rx buffer near-full and forces window updates.
            let mut progressed = false;
            {
                let mut c = sockets.get::<TcpSocket>(ch);
                if c.can_recv() {
                    if let Ok(n) = c.recv_slice(&mut rbuf) {
                        for i in 0..n {
                            assert_eq!(
                                rbuf[i],
                                (recvd + i) as u8,
                                "data corruption at byte {}",
                                recvd + i
                            );
                        }
                        if n > 0 {
                            recvd += n;
                            progressed = true;
                        }
                    }
                }
            }

            idle = if progressed { 0 } else { idle + 1 };
            assert!(
                idle < 5_000_000,
                "TRANSFER STALLED: recvd={} of {} (sent={}) — window-update deadlock",
                recvd,
                total,
                sent
            );
        }
        assert_eq!(recvd, total, "did not receive the whole stream");
    }

    /// 16 MiB through 2 MiB buffers with a 16 KiB/poll reader: the buffer fills
    /// ~8 times over and the window cycles closed/open continuously.
    #[test]
    fn large_transfer_slow_reader() {
        run_transfer(16 * 1024 * 1024, 16 * 1024);
    }

    /// A faster reader (256 KiB/poll) — should breeze through; guards against a
    /// regression where even unthrottled large transfers stall.
    #[test]
    fn large_transfer_fast_reader() {
        run_transfer(16 * 1024 * 1024, 256 * 1024);
    }
}

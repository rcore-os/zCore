//! IO Multiplex operations
//!
//! - select, pselect
//! - poll, ppoll

use super::*;
use alloc::vec::Vec;
use bitvec::prelude::{BitVec, Lsb0};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use kernel_hal::timer;
use linux_object::fs::{FileDesc, PollEvents};
use linux_object::time::*;

/// Monotonic time since boot — must match `timer::timer_set` deadlines (not wall clock).
fn mono_now() -> Duration {
    timer::timer_now()
}

fn schedule_poll_wakeup(
    cx: &mut Context,
    after: Duration,
    timer_token: &mut Option<kernel_hal::timer_waker::TimerWakerSlot>,
) {
    let deadline = mono_now() + after;
    // Refresh in place while the previous tick is still pending; re-arm only
    // after it fired. Avoids AtomicBool TOCTOU + timer-heap churn.
    kernel_hal::timer_waker::ensure_timer_waker(timer_token, deadline, cx);
}

fn kill_poll_timer(timer_token: &mut Option<kernel_hal::timer_waker::TimerWakerSlot>) {
    kernel_hal::timer_waker::kill_timer_waker(timer_token);
}

/// Wakeup granularity for select/poll/epoll (IRQ wakes can arrive earlier).
const IO_WAIT_TICK: Duration = Duration::from_millis(linux_object::net::wait::IO_WAIT_TICK_MS);

/// Slow re-poll granularity for an interactive wait whose terminal is NOT the
/// active VT. A background-VT shell sits in poll(stdin) for keyboard input that
/// can only ever arrive on the *active* terminal, so re-polling it at the fast
/// 4 ms tick just burns CPU (the busy/heat with several spare VT shells). Poll
/// it slowly instead; it still wakes immediately on a real event, and within
/// this bound it notices its VT becoming active and resumes fast polling.
const SLOW_IO_WAIT_TICK: Duration = Duration::from_millis(100);

/// Pick the io-wait re-poll interval. The slow tick exists for exactly one
/// pattern: a shell parked in poll(stdin) on a *background* VT, whose input can
/// only ever arrive once its VT becomes active — re-polling that at 4 ms just
/// burns CPU. Everything else gets the fast tick: in particular a poll set with
/// *no* interactive fd at all (DRM fds, timerfds, pipes, device fds — the shape
/// of a compositor's startup waits) must NOT be demoted to 100 ms, or every
/// such roundtrip is gated at a tenth of a second and startup takes minutes.
fn io_wait_interval(s: &Syscall, watch_net: bool, watch_interactive: bool) -> Duration {
    let background_interactive =
        watch_interactive && s.linux_process().vt() != kernel_hal::console::active_vt();
    if !watch_net && background_interactive {
        SLOW_IO_WAIT_TICK
    } else {
        IO_WAIT_TICK
    }
}

fn arm_io_wait(cx: &mut Context, watch_net: bool, watch_interactive: bool, io_armed: &mut bool) {
    if *io_armed {
        linux_object::net::retain_io_wait_wakers(cx.waker(), watch_net, watch_interactive);
        *io_armed = false;
        return;
    }
    linux_object::net::register_io_wait_wakers(cx.waker(), watch_net, watch_interactive);
    *io_armed = true;
}

fn clear_poll_io(
    timer: &mut Option<kernel_hal::timer_waker::TimerWakerSlot>,
    io_waker: &mut Option<core::task::Waker>,
    watch_net: bool,
    watch_interactive: bool,
) {
    kill_poll_timer(timer);
    if let Some(w) = io_waker.take() {
        linux_object::net::clear_io_wait_wakers(&w, watch_net, watch_interactive);
    }
}

impl Syscall<'_> {
    /// Wait for some event on a file descriptor
    pub async fn sys_poll(
        &mut self,
        mut ufds: UserInOutPtr<PollFd>,
        nfds: usize,
        timeout_msecs: isize,
    ) -> SysResult {
        let _ = self.maybe_handle_tty_intr()?;
        let mut polls = ufds.read_array(nfds)?;
        info!(
            "poll: ufds: {:?}, nfds: {:?}, timeout_msecs: {}",
            polls, nfds, timeout_msecs
        );

        let begin_time = mono_now();
        #[must_use = "future does nothing unless polled/`await`-ed"]
        struct PollFuture<'a> {
            polls: &'a mut Vec<PollFd>,
            timeout_msecs: isize,
            begin_time: Duration,
            syscall: &'a Syscall<'a>,
            io_armed: bool,
            timer: Option<kernel_hal::timer_waker::TimerWakerSlot>,
            /// Last watch flags + waker parked in IRQ lists (for Drop cleanup).
            watch_net: bool,
            watch_interactive: bool,
            io_waker: Option<core::task::Waker>,
            /// Readiness wakers parked on the watched fds' event buses
            /// (pipes, unix sockets, ptys, eventfd/timerfd, DRM). Refreshed
            /// each pass; RAII-unsubscribed on drop.
            subs: Vec<linux_object::sync::ReadinessSub>,
        }
        impl Drop for PollFuture<'_> {
            fn drop(&mut self) {
                let wn = self.watch_net; let wi = self.watch_interactive; clear_poll_io(&mut self.timer, &mut self.io_waker, wn, wi);
            }
        }
        impl<'a> Future for PollFuture<'a> {
            type Output = SysResult;

            fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                use PollEvents as PE;
                let this = self.get_mut();
                // Unsubscribe last pass's readiness wakers before re-scanning;
                // fresh ones are parked below if we wait again.
                this.subs.clear();
                if let Err(e) = linux_object::process::check_signals() {
                    clear_poll_io(
                        &mut this.timer,
                        &mut this.io_waker,
                        this.watch_net,
                        this.watch_interactive,
                    );
                    return Poll::Ready(Err(e));
                }
                let watch_net = this
                    .polls
                    .iter()
                    .any(|p| linux_object::net::fd_is_socket(p.fd));
                let watch_interactive = this
                    .polls
                    .iter()
                    .any(|p| linux_object::net::fd_is_interactive(p.fd));
                this.watch_net = watch_net;
                this.watch_interactive = watch_interactive;
                if this.io_armed {
                    arm_io_wait(cx, watch_net, watch_interactive, &mut this.io_armed);
                }
                linux_object::net::io_wait_tick(watch_net, watch_interactive);
                let proc = this.syscall.linux_process();
                let mut events = 0;
                let mut early_err = None;

                // iterate each poll to check whether it is ready
                for poll in this.polls.iter_mut() {
                    poll.revents = PE::empty();
                    if let Ok(file_like) = proc.get_file_like(poll.fd) {
                        debug!("get file like: {:?}", file_like);
                        // Sync poll only — see Epoll::wait. async_poll per fd
                        // here nested huge futures on the coroutine stack and
                        // #DF'd `__from_user` when labwc/libinput started.
                        let status = match file_like.poll(poll.events) {
                            Ok(ret) => ret,
                            Err(err) => {
                                debug!("poll ret err: {:?}", err);
                                early_err = Some(err);
                                break;
                            }
                        };
                        if status.error {
                            poll.revents |= PE::ERR;
                            events += 1;
                        }
                        if status.read && poll.events.contains(PE::IN) {
                            poll.revents |= PE::IN;
                            events += 1;
                        }
                        if status.write && poll.events.contains(PE::OUT) {
                            poll.revents |= PE::OUT;
                            events += 1;
                        }
                    } else if <FileDesc as Into<i32>>::into(poll.fd) < 0 {
                        // POSIX poll(2): negative fds are ignored (udhcpc6 leaves -1 in the set).
                        poll.revents = PE::empty();
                    } else {
                        // POLLNVAL is a well-defined answer, not a kernel
                        // error; keep the diagnostic off the (synchronous,
                        // slow) warn channel — a program polling a stale fd in
                        // a loop would stall on serial output otherwise.
                        debug!("can not find filelike object from fd: {:?}", poll.fd);
                        poll.revents |= PE::INVAL;
                        events += 1;
                    }
                }
                if let Some(err) = early_err {
                    clear_poll_io(
                        &mut this.timer,
                        &mut this.io_waker,
                        watch_net,
                        watch_interactive,
                    );
                    return Poll::Ready(Err(err));
                }
                // some event happens, so evoke the process
                if events > 0 {
                    clear_poll_io(
                        &mut this.timer,
                        &mut this.io_waker,
                        watch_net,
                        watch_interactive,
                    );
                    return Poll::Ready(Ok(events));
                }

                // Nothing ready and we are about to wait: park a readiness
                // waker on every fd that can carry one (flat registration —
                // see `FileLike::subscribe_readiness`), so a pipe write or a
                // unix-socket send wakes this task the moment it happens
                // instead of on the next re-scan tick. Refreshed every pass
                // (stale subscriptions were dropped by the `clear()` above);
                // events racing in after the scan are caught by the
                // EventBus's latched flags, which fire the waker at
                // subscribe time. With full coverage the backstop stretches
                // from 4 ms to the covered tick; any unsubscribable fd keeps
                // the short tick for the whole set.
                let mut covered = false;
                if this.timeout_msecs != 0 {
                    covered = !this.polls.is_empty();
                    for p in this.polls.iter() {
                        if <FileDesc as Into<i32>>::into(p.fd) < 0 {
                            continue; // ignored slot (POSIX): nothing to wake on
                        }
                        match proc
                            .get_file_like(p.fd)
                            .ok()
                            .and_then(|f| f.subscribe_readiness(p.events, cx.waker()))
                        {
                            Some(sub) => this.subs.push(sub),
                            None => covered = false,
                        }
                    }
                }
                let covered_tick =
                    Duration::from_millis(linux_object::net::wait::IO_WAIT_COVERED_TICK_MS);

                match this.timeout_msecs {
                    // no timeout, return now;
                    0 => {
                        clear_poll_io(
                            &mut this.timer,
                            &mut this.io_waker,
                            watch_net,
                            watch_interactive,
                        );
                        return Poll::Ready(Ok(0));
                    }
                    1.. => {
                        let deadline =
                            this.begin_time + Duration::from_millis(this.timeout_msecs as u64);
                        if mono_now() >= deadline {
                            clear_poll_io(
                                &mut this.timer,
                                &mut this.io_waker,
                                watch_net,
                                watch_interactive,
                            );
                            return Poll::Ready(Ok(0));
                        }
                        let remaining = deadline.saturating_sub(mono_now());
                        let tick = if covered {
                            covered_tick
                        } else {
                            io_wait_interval(this.syscall, watch_net, watch_interactive)
                        };
                        let wake_in = remaining.min(tick);
                        arm_io_wait(cx, watch_net, watch_interactive, &mut this.io_armed);
                        this.io_waker = Some(cx.waker().clone());
                        schedule_poll_wakeup(cx, wake_in, &mut this.timer);
                    }
                    -1 => {
                        let tick = if covered {
                            covered_tick
                        } else {
                            io_wait_interval(this.syscall, watch_net, watch_interactive)
                        };
                        arm_io_wait(cx, watch_net, watch_interactive, &mut this.io_armed);
                        this.io_waker = Some(cx.waker().clone());
                        schedule_poll_wakeup(cx, tick, &mut this.timer);
                    }
                    _ => {
                        info!("No waker. timeout: {:?}", this.timeout_msecs);
                    }
                }

                Poll::Pending
            }
        }

        let future = PollFuture {
            polls: &mut polls,
            timeout_msecs,
            begin_time,
            syscall: self,
            io_armed: false,
            timer: None,
            watch_net: false,
            watch_interactive: false,
            io_waker: None,
            subs: Vec::new(),
        };
        let result = future.await;
        ufds.write_array(&polls)?;
        info!("return ufds: {:?}", polls);
        result
    }

    /// Wait for some event on a file descriptor
    ///
    /// ppoll() allows an application to safely wait until either a file descriptor becomes ready or until a signal is caught
    pub async fn sys_ppoll(
        &mut self,
        ufds: UserInOutPtr<PollFd>,
        nfds: usize,
        timeout: UserInPtr<TimeSpec>,
    ) -> SysResult {
        let timeout_msecs = if timeout.is_null() {
            -1
        } else {
            let timeout = timeout.read()?;
            info!("sys_ppoll: timeout: {:?}", timeout);
            timeout.to_msec() as isize
        };

        self.sys_poll(ufds, nfds, timeout_msecs).await
    }

    /// similar to select, but have sigmask argument
    pub async fn sys_pselect6(
        &mut self,
        nfds: usize,
        read: UserInOutPtr<u32>,
        write: UserInOutPtr<u32>,
        err: UserInOutPtr<u32>,
        timeout: UserInPtr<TimeSpec>,
        _sigset: usize,
    ) -> SysResult {
        // pselect6's timeout is a `timespec` (NANOseconds). It was previously
        // parsed as select(2)'s `timeval` (MICROseconds), inflating every
        // timeout by 1000x: glibc routes plain select() through pselect6, so a
        // 100 ms select slept 100 SECONDS. busybox ash's line editor and every
        // terminal-probe wait sat "wedged" exactly this way.
        let timeout_msecs = if timeout.is_null() {
            -1
        } else {
            timeout.read()?.to_msec() as isize
        };
        self.select_core(nfds, read, write, err, timeout_msecs)
            .await
    }

    /// allow a program to monitor multiple file descriptors,
    /// waiting until one or more of the file descriptors become "ready" for some class of I/O operation.
    ///
    /// A file descriptor is considered ready if it is possible to perform the corresponding I/O operation (e.g., read) without blocking.
    pub async fn sys_select(
        &mut self,
        nfds: usize,
        read: UserInOutPtr<u32>,
        write: UserInOutPtr<u32>,
        err: UserInOutPtr<u32>,
        timeout: UserInPtr<TimeVal>,
    ) -> SysResult {
        let _ = self.maybe_handle_tty_intr()?;
        info!(
            "select: nfds: {}, read: {:?}, write: {:?}, err: {:?}, timeout: {:?}",
            nfds, read, write, err, timeout
        );
        /* nfds = 0 is a valid way to sleep in POSIX
        if nfds as u64 == 0 {
            return Ok(0);
        } */
        let timeout_msecs = if !timeout.is_null() {
            let timeout = timeout.read()?;
            timeout.to_msec() as isize
        } else {
            // infinity
            -1
        };
        self.select_core(nfds, read, write, err, timeout_msecs)
            .await
    }

    /// Shared body of `select`/`pselect6` once the timeout is normalized to
    /// milliseconds (`-1` = infinite). The two entry points differ only in the
    /// timeout's user-space type: `timeval` (us) vs `timespec` (ns).
    pub async fn select_core(
        &mut self,
        nfds: usize,
        read: UserInOutPtr<u32>,
        write: UserInOutPtr<u32>,
        err: UserInOutPtr<u32>,
        timeout_msecs: isize,
    ) -> SysResult {
        let mut read_fds = FdSet::new(read, nfds)?;
        let mut write_fds = FdSet::new(write, nfds)?;
        let mut err_fds = FdSet::new(err, nfds)?;
        let begin_time = mono_now();

        // The select set membership (`origin`) does not change while the future
        // is being polled, so whether we need to pump the network / interactive
        // I/O is invariant. Compute it once here instead of on every wakeup.
        let watch_net = (0..nfds).any(|fd| {
            fd >= linux_object::net::SOCKET_FD
                && (read_fds.contains(FileDesc::from(fd))
                    || write_fds.contains(FileDesc::from(fd))
                    || err_fds.contains(FileDesc::from(fd)))
        });
        let watch_interactive = (0..nfds).any(|fd| {
            linux_object::net::fd_is_interactive(FileDesc::from(fd))
                && (read_fds.contains(FileDesc::from(fd))
                    || write_fds.contains(FileDesc::from(fd))
                    || err_fds.contains(FileDesc::from(fd)))
        });

        #[must_use = "future does nothing unless polled/`await`-ed"]
        struct SelectFuture<'a> {
            read_fds: &'a mut FdSet,
            write_fds: &'a mut FdSet,
            err_fds: &'a mut FdSet,
            nfds: usize,
            watch_net: bool,
            watch_interactive: bool,
            timeout_msecs: isize,
            begin_time: Duration,
            syscall: &'a Syscall<'a>,
            io_armed: bool,
            timer: Option<kernel_hal::timer_waker::TimerWakerSlot>,
            io_waker: Option<core::task::Waker>,
            /// Readiness wakers parked on the watched fds (see PollFuture).
            subs: Vec<linux_object::sync::ReadinessSub>,
        }

        impl Drop for SelectFuture<'_> {
            fn drop(&mut self) {
                let wn = self.watch_net; let wi = self.watch_interactive; clear_poll_io(&mut self.timer, &mut self.io_waker, wn, wi);
            }
        }

        impl<'a> Future for SelectFuture<'a> {
            type Output = SysResult;

            fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                let this = self.get_mut();
                // Unsubscribe last pass's readiness wakers before re-scanning.
                this.subs.clear();
                if let Err(e) = linux_object::process::check_signals() {
                    clear_poll_io(
                        &mut this.timer,
                        &mut this.io_waker,
                        this.watch_net,
                        this.watch_interactive,
                    );
                    return Poll::Ready(Err(e));
                }
                let watch_net = this.watch_net;
                let watch_interactive = this.watch_interactive;
                if this.io_armed {
                    arm_io_wait(cx, watch_net, watch_interactive, &mut this.io_armed);
                }
                linux_object::net::io_wait_tick(watch_net, watch_interactive);
                let files = this.syscall.linux_process().get_files()?;

                let mut events = 0;
                let mut early_err = None;
                // Iterate only the fds in the select set instead of every open
                // fd in the process.
                for fd in 0..this.nfds {
                    let fd = FileDesc::from(fd);
                    if !this.err_fds.contains(fd)
                        && !this.read_fds.contains(fd)
                        && !this.write_fds.contains(fd)
                    {
                        continue;
                    }
                    let file_like = match files.get(&fd) {
                        Some(f) => f,
                        None => continue,
                    };
                    // Sync poll only — same stack-overflow rationale as sys_poll /
                    // Epoll::wait (labwc bring-up #DF in `__from_user`).
                    let status = match file_like.poll(PollEvents::all()) {
                        Ok(ret) => ret,
                        Err(err) => {
                            early_err = Some(err);
                            break;
                        }
                    };
                    if status.error && this.err_fds.contains(fd) {
                        this.err_fds.set(fd);
                        events += 1;
                    }
                    if status.read && this.read_fds.contains(fd) {
                        this.read_fds.set(fd);
                        events += 1;
                    }
                    if status.write && this.write_fds.contains(fd) {
                        this.write_fds.set(fd);
                        events += 1;
                    }
                }
                if let Some(err) = early_err {
                    clear_poll_io(
                        &mut this.timer,
                        &mut this.io_waker,
                        watch_net,
                        watch_interactive,
                    );
                    return Poll::Ready(Err(err));
                }

                // some event happens, so evoke the process
                if events > 0 {
                    clear_poll_io(
                        &mut this.timer,
                        &mut this.io_waker,
                        watch_net,
                        watch_interactive,
                    );
                    // Flush the ready bitmaps to user space once.
                    this.read_fds.commit();
                    this.write_fds.commit();
                    this.err_fds.commit();
                    return Poll::Ready(Ok(events));
                }

                // Same readiness-subscription scheme as PollFuture: park a
                // waker per watched fd, stretch the backstop when every fd
                // took one.
                let mut covered = false;
                if this.timeout_msecs != 0 {
                    covered = true;
                    let mut any_watched = false;
                    for fd in 0..this.nfds {
                        let fd = FileDesc::from(fd);
                        let mut interest = PollEvents::empty();
                        if this.read_fds.contains(fd) {
                            interest |= PollEvents::IN;
                        }
                        if this.write_fds.contains(fd) {
                            interest |= PollEvents::OUT;
                        }
                        if interest.is_empty() && !this.err_fds.contains(fd) {
                            continue;
                        }
                        any_watched = true;
                        match files
                            .get(&fd)
                            .and_then(|f| f.subscribe_readiness(interest, cx.waker()))
                        {
                            Some(sub) => this.subs.push(sub),
                            None => covered = false,
                        }
                    }
                    covered &= any_watched;
                }
                let covered_tick =
                    Duration::from_millis(linux_object::net::wait::IO_WAIT_COVERED_TICK_MS);

                match this.timeout_msecs {
                    // no timeout, return now;
                    0 => {
                        clear_poll_io(
                            &mut this.timer,
                            &mut this.io_waker,
                            watch_net,
                            watch_interactive,
                        );
                        return Poll::Ready(Ok(0));
                    }
                    1.. => {
                        let deadline =
                            this.begin_time + Duration::from_millis(this.timeout_msecs as u64);
                        if mono_now() >= deadline {
                            clear_poll_io(
                                &mut this.timer,
                                &mut this.io_waker,
                                watch_net,
                                watch_interactive,
                            );
                            return Poll::Ready(Ok(0));
                        }
                        let remaining = deadline.saturating_sub(mono_now());
                        let tick = if covered {
                            covered_tick
                        } else {
                            io_wait_interval(this.syscall, watch_net, watch_interactive)
                        };
                        let wake_in = remaining.min(tick);
                        arm_io_wait(cx, watch_net, watch_interactive, &mut this.io_armed);
                        this.io_waker = Some(cx.waker().clone());
                        schedule_poll_wakeup(cx, wake_in, &mut this.timer);
                    }
                    -1 => {
                        let tick = if covered {
                            covered_tick
                        } else {
                            io_wait_interval(this.syscall, watch_net, watch_interactive)
                        };
                        arm_io_wait(cx, watch_net, watch_interactive, &mut this.io_armed);
                        this.io_waker = Some(cx.waker().clone());
                        schedule_poll_wakeup(cx, tick, &mut this.timer);
                    }
                    _ => {}
                }
                Poll::Pending
            }
        }
        let future = SelectFuture {
            read_fds: &mut read_fds,
            write_fds: &mut write_fds,
            err_fds: &mut err_fds,
            nfds,
            watch_net,
            watch_interactive,
            timeout_msecs,
            begin_time,
            syscall: self,
            io_armed: false,
            timer: None,
            io_waker: None,
            subs: Vec::new(),
        };
        future.await
    }

    /// creates an epoll instance
    pub fn sys_epoll_create1(&self, flags: usize) -> SysResult {
        info!("epoll_create1: flags={:#x}", flags);
        let proc = self.linux_process();
        let epoll = Epoll::new(OpenFlags::from_bits_truncate(flags));
        let fd = proc.add_file(epoll)?;
        Ok(fd.into())
    }

    /// opens an epoll file descriptor
    pub fn sys_epoll_create(&self, size: usize) -> SysResult {
        info!("epoll_create: size={}", size);
        self.sys_epoll_create1(0)
    }

    /// control interface for an epoll file descriptor
    pub fn sys_epoll_ctl(
        &self,
        epfd: FileDesc,
        op: i32,
        fd: FileDesc,
        event: UserInPtr<EpollEvent>,
    ) -> SysResult {
        info!(
            "epoll_ctl: epfd={:?}, op={}, fd={:?}, event={:?}",
            epfd, op, fd, event
        );
        let proc = self.linux_process();
        let epoll_file = proc.get_file_like(epfd)?;
        let epoll = epoll_file.downcast_ref::<Epoll>().ok_or(LxError::EBADF)?;
        let (event, file) = if op == 2 {
            // EPOLL_CTL_DEL: no event payload, no file handle needed.
            (EpollEvent { events: 0, data: 0 }, None)
        } else {
            // ADD/MOD: resolve the target fd so the epoll can poll it directly
            // (required for nested-epoll readiness).
            (event.read()?, Some(proc.get_file_like(fd)?))
        };
        epoll.ctl(op, fd, event, file)
    }

    /// wait for an I/O event on an epoll file descriptor
    pub async fn sys_epoll_pwait(
        &self,
        epfd: FileDesc,
        mut events: UserOutPtr<EpollEvent>,
        maxevents: usize,
        timeout: isize,
        _sigmask: usize,
    ) -> SysResult {
        log::trace!(
            "epoll_pwait: epfd={:?}, maxevents={}, timeout={}",
            epfd,
            maxevents,
            timeout
        );
        // Resolve the epoll object to an owned Arc (not a borrow of a local):
        // `wait` awaits, and a stale net/timer waker re-polling this future
        // after teardown must not dereference a freed process/file. The Arc
        // keeps the epoll object itself alive for the whole wait; `wait`
        // likewise holds each watched file by Arc, so the future carries no
        // reference that outlives what it points at.
        let epoll = self
            .linux_process()
            .get_file_like(epfd)?
            .downcast_arc::<Epoll>()
            .map_err(|_| LxError::EBADF)?;

        // TODO: handle timeout
        let res_events = epoll.wait(maxevents, timeout).await?;
        events.write_array(&res_events)?;
        Ok(res_events.len())
    }

    /// wait for an I/O event on an epoll file descriptor
    pub async fn sys_epoll_wait(
        &self,
        epfd: FileDesc,
        events: UserOutPtr<EpollEvent>,
        maxevents: usize,
        timeout: isize,
    ) -> SysResult {
        self.sys_epoll_pwait(epfd, events, maxevents, timeout, 0)
            .await
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct PollFd {
    fd: FileDesc,
    events: PollEvents,
    revents: PollEvents,
}

/// fd size per item
const FD_PER_ITEM: usize = u32::BITS as usize;
/// max Fdset size
const MAX_FDSET_SIZE: usize = 1024 / FD_PER_ITEM;

/// FdSet data struct for select
struct FdSet {
    /// input addr, for update Fdset use
    addr: UserInOutPtr<u32>,
    /// FdSet bit buffer
    origin: BitVec<Lsb0, u32>,
    /// Ready bit buffer
    ready: BitVec<Lsb0, u32>,
}

impl FdSet {
    /// Initialize a `FdSet` from pointer and number of fds
    /// Check if the array is large enough
    fn new(mut addr: UserInOutPtr<u32>, nfds: usize) -> Result<FdSet, LxError> {
        if addr.is_null() {
            Ok(FdSet {
                addr,
                origin: BitVec::new(),
                ready: BitVec::new(),
            })
        } else {
            let len = nfds.div_ceil(FD_PER_ITEM);
            if len > MAX_FDSET_SIZE {
                return Err(LxError::EINVAL);
            }
            // save the fdset, and clear it
            let origin = BitVec::from_slice(addr.as_slice(len)?).unwrap();
            let vec0 = vec![0; len];
            addr.write_array(&vec0)?;
            let ready = BitVec::from_slice(&vec0).unwrap();
            Ok(FdSet {
                addr,
                origin,
                ready,
            })
        }
    }

    /// Mark `fd` as ready in this `FdSet`.
    ///
    /// This only updates the in-memory bitmap; the result is flushed to user
    /// space once via [`commit`](Self::commit) instead of rewriting the whole
    /// bit buffer on every ready fd.
    /// Fd should be less than nfds
    fn set(&mut self, fd: FileDesc) {
        let fd: usize = fd.into();
        if fd < self.ready.len() {
            self.ready.set(fd, true);
        }
    }

    /// Write the ready bitmap back to user memory once.
    fn commit(&mut self) {
        if self.ready.is_empty() {
            return;
        }
        let vec: Vec<u32> = self.ready.clone().into();
        let _ = self.addr.write_array(&vec);
    }

    /// Check to see whether `fd` is in original `FdSet`
    /// Fd should be less than nfds
    fn contains(&self, fd: FileDesc) -> bool {
        let fd: usize = fd.into();
        if fd < self.origin.len() {
            self.origin[fd]
        } else {
            false
        }
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod abi_tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn pollfd_matches_linux_uapi() {
        assert_eq!(size_of::<PollFd>(), 8);
    }
}

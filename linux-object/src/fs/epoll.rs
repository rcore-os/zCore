use super::*;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use lock::Mutex;
use zircon_object::object::*;

/// Max nesting depth for one epoll watching another (mirrors Linux's
/// `EP_MAX_NESTS`). Also bounds the cycle-detection walk in
/// `Epoll::contains_epoll` itself, so a bug elsewhere that let a cycle slip
/// past this check couldn't make the check's own recursion unbounded either.
const EPOLL_MAX_NEST_DEPTH: usize = 4;

lazy_static::lazy_static! {
    /// Serializes EPOLL_CTL_ADD of a nested epoll (one epoll fd watching
    /// another) so the cycle/depth check and the insert happen as one atomic
    /// step. Without this, two concurrent adds in opposite directions
    /// (thread 1: add B to A; thread 2: add A to B) could each pass
    /// `contains_epoll` before either inserts, still creating a cycle.
    /// Mirrors Linux's global `epmutex`, used for the same reason.
    static ref EPOLL_NEST_LOCK: Mutex<()> = Mutex::new(());
}

/// epoll implementation
pub struct Epoll {
    base: KObjectBase,
    inner: Mutex<EpollInner>,
    flags: OpenFlags,
}

struct EpollInner {
    /// Each watched fd maps to its requested event mask plus a handle to the
    /// underlying file. The file handle lets `Epoll::poll()` report readiness
    /// of the watched fds *without* a process context — which is what makes a
    /// nested epoll (an epoll fd added to another epoll's interest list) work.
    /// wlroots/labwc relies on exactly this: it adds `libinput_get_fd()` (an
    /// epoll fd) to its `wl_event_loop` epoll, so the outer epoll must surface
    /// the inner epoll's readiness or input events are never dispatched.
    interest_list: BTreeMap<FileDesc, (EpollEvent, Arc<dyn FileLike>)>,
}

/// epoll event
#[repr(C)]
#[cfg_attr(target_arch = "x86_64", repr(packed))]
#[derive(Clone, Copy, Debug)]
pub struct EpollEvent {
    /// events
    pub events: u32,
    /// data
    pub data: u64,
}

impl_kobject!(Epoll);

impl Epoll {
    /// create an epoll instance
    pub fn new(flags: OpenFlags) -> Arc<Self> {
        Arc::new(Epoll {
            base: KObjectBase::new(),
            inner: Mutex::new(EpollInner {
                interest_list: BTreeMap::new(),
            }),
            flags,
        })
    }

    /// add, modify, or remove a file descriptor from the interest list. `file`
    /// is the resolved handle for `fd` (required for ADD/MOD; ignored for DEL).
    pub fn ctl(
        &self,
        op: i32,
        fd: FileDesc,
        event: EpollEvent,
        file: Option<Arc<dyn FileLike>>,
    ) -> LxResult<usize> {
        let mut inner = self.inner.lock();
        match op {
            1 => {
                // EPOLL_CTL_ADD
                if inner.interest_list.contains_key(&fd) {
                    return Err(LxError::EEXIST);
                }
                let file = file.ok_or(LxError::EBADF)?;
                // If the target is itself an epoll, reject a self-add or any
                // nesting that would create a cycle or exceed
                // EPOLL_MAX_NEST_DEPTH. any_ready()'s recursion through
                // file.poll() has no other terminating condition -- a cycle
                // recurses forever, and even an acyclic chain deep enough can
                // overflow a coroutine's guard-page-less stack (the same bug
                // class root-caused once already for unbounded path
                // recursion, commit b448c77e).
                if let Ok(target) = file.clone().downcast_arc::<Epoll>() {
                    // Serialize against a concurrent nested add elsewhere so
                    // the check and the insert are atomic as a unit (see
                    // EPOLL_NEST_LOCK's doc comment) -- drop our own lock
                    // first since contains_epoll never needs to re-lock
                    // `self` (it returns as soon as it reaches `self` by
                    // pointer, before locking that node), but a *different*
                    // epoll's `ctl` could still be walking through us.
                    drop(inner);
                    let _nest_guard = EPOLL_NEST_LOCK.lock();
                    if target.contains_epoll(self, 0) {
                        return Err(LxError::ELOOP);
                    }
                    inner = self.inner.lock();
                    if inner.interest_list.contains_key(&fd) {
                        return Err(LxError::EEXIST);
                    }
                }
                inner.interest_list.insert(fd, (event, file));
            }
            2 => {
                // EPOLL_CTL_DEL
                inner.interest_list.remove(&fd).ok_or(LxError::ENOENT)?;
            }
            3 => {
                // EPOLL_CTL_MOD
                let file = file.ok_or(LxError::EBADF)?;
                let e = inner.interest_list.get_mut(&fd).ok_or(LxError::ENOENT)?;
                *e = (event, file);
            }
            _ => return Err(LxError::EINVAL),
        }
        Ok(0)
    }

    /// Whether `needle` (some other epoll, compared by identity) is
    /// reachable by walking outward from `self` through nested epolls --
    /// i.e. whether `self` already (transitively) watches `needle`. Used by
    /// `ctl`'s ADD path to reject a nesting that would create a cycle.
    /// `depth >= EPOLL_MAX_NEST_DEPTH` conservatively answers "yes" (refuse
    /// rather than risk missing a cycle further down) rather than growing
    /// unbounded itself.
    fn contains_epoll(&self, needle: *const Epoll, depth: usize) -> bool {
        if core::ptr::eq(self, needle) {
            return true;
        }
        if depth >= EPOLL_MAX_NEST_DEPTH {
            return true;
        }
        // Snapshot then drop the lock before recursing, same reasoning as
        // any_ready(): a watched fd can itself be an epoll that re-enters.
        let entries: Vec<Arc<dyn FileLike>> = self
            .inner
            .lock()
            .interest_list
            .values()
            .map(|(_, f)| f.clone())
            .collect();
        for f in entries {
            if let Ok(child) = f.downcast_arc::<Epoll>() {
                if child.contains_epoll(needle, depth + 1) {
                    return true;
                }
            }
        }
        false
    }

    /// Returns whether any watched fd is currently ready for its requested
    /// events. Shared by `poll`/`async_poll` so a nested epoll surfaces its
    /// inner readiness to an outer epoll/poll.
    ///
    /// Nested epolls (libinput's fd inside wlroots) are walked **iteratively**
    /// with an explicit heap stack — recursive `file.poll() → any_ready()`
    /// stacked a fresh interest-list snapshot frame per nest level on the
    /// coroutine stack during labwc bring-up.
    fn any_ready(&self) -> bool {
        let mut stack: Vec<(EpollEvent, Arc<dyn FileLike>, usize)> = self
            .inner
            .lock()
            .interest_list
            .values()
            .cloned()
            .map(|(e, f)| (e, f, 0))
            .collect();
        while let Some((event, file, depth)) = stack.pop() {
            if let Ok(child) = file.clone().downcast_arc::<Epoll>() {
                if depth >= EPOLL_MAX_NEST_DEPTH {
                    continue;
                }
                let nested: Vec<(EpollEvent, Arc<dyn FileLike>, usize)> = child
                    .inner
                    .lock()
                    .interest_list
                    .values()
                    .cloned()
                    .map(|(e, f)| (e, f, depth + 1))
                    .collect();
                stack.extend(nested);
                continue;
            }
            let interest = PollEvents::from_bits_truncate(event.events as u16);
            if let Ok(status) = file.poll(interest) {
                if (status.read && interest.contains(PollEvents::IN))
                    || (status.write && interest.contains(PollEvents::OUT))
                    || status.error
                {
                    return true;
                }
            }
        }
        false
    }
}

#[async_trait]
impl FileLike for Epoll {
    fn flags(&self) -> OpenFlags {
        self.flags
    }

    fn set_flags(&self, _f: OpenFlags) -> LxResult {
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            inner: Mutex::new(EpollInner {
                interest_list: self.inner.lock().interest_list.clone(),
            }),
            flags: self.flags,
        })
    }

    async fn read(&self, _buf: &mut [u8]) -> LxResult<usize> {
        Err(LxError::ENOSYS)
    }

    fn write(&self, _buf: &[u8]) -> LxResult<usize> {
        Err(LxError::ENOSYS)
    }

    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
        Err(LxError::ENOSYS)
    }

    fn poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        // An epoll fd is readable iff any watched fd is ready. Surfacing this is
        // what lets a nested epoll (e.g. libinput's fd inside wlroots' event
        // loop) wake an outer epoll/poll.
        Ok(PollStatus {
            read: self.any_ready(),
            write: false,
            error: false,
        })
    }

    async fn async_poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        Ok(PollStatus {
            read: self.any_ready(),
            write: false,
            error: false,
        })
    }
}

impl Epoll {
    /// wait for events on the interest list
    pub async fn wait(&self, maxevents: usize, timeout_msecs: isize) -> LxResult<Vec<EpollEvent>> {
        let begin_time = kernel_hal::timer::timer_now();
        loop {
            crate::process::check_signals()?;
            // Snapshot the interest list, keeping each watched file's OWN
            // `Arc<dyn FileLike>`. Two reasons this is the handle to poll,
            // rather than re-resolving the fd number through the process:
            //  * Correctness — epoll watches the open file *description*, not
            //    the fd. If userspace closes the fd and reuses the number for
            //    something else, Linux keeps reporting the original
            //    description until EPOLL_CTL_DEL; re-resolving by number
            //    watched the wrong file.
            //  * Lifetime — polling the stored Arc means this future holds NO
            //    `&LinuxProcess` borrow across its `.await` points. The borrow
            //    reached the process through the thread, and a stale
            //    net/timer waker re-polling a parked epoll after the owning
            //    thread had torn down dereferenced freed process memory
            //    (a use-after-free #GP with the free-poison pattern in the
            //    faulting register). The Arc keeps exactly what we touch
            //    alive for as long as we touch it.
            let interest_list: Vec<(FileDesc, EpollEvent, Arc<dyn FileLike>)> = self
                .inner
                .lock()
                .interest_list
                .iter()
                .map(|(fd, (event, file))| (*fd, *event, file.clone()))
                .collect();
            let watch_net = interest_list
                .iter()
                .any(|(fd, _, _)| crate::net::fd_is_socket(*fd));
            let watch_interactive = interest_list
                .iter()
                .any(|(fd, _, _)| crate::net::fd_is_interactive(*fd));
            crate::net::io_wait_tick(watch_net, watch_interactive);
            // Sync readiness scan. Do NOT call `async_poll` here: each scan used
            // to Box::pin+poll+drop a future per watched fd (libinput epoll in
            // labwc touches dozens). The futures never stayed parked across
            // the await below anyway (Dropped before IoMultiplexWait), but
            // constructing them blew the guard-page-less coroutine stack → #DF
            // in `__from_user` at session start. Wakeups come from the 4 ms
            // IoMultiplexWait tick (and HID/NET IRQ registration there).
            let mut events = Vec::new();
            for (_fd, event, file) in &interest_list {
                let interest = PollEvents::from_bits_truncate(event.events as u16);
                let status = match file.poll(interest) {
                    Ok(status) => status,
                    Err(err) => return Err(err),
                };
                let mut ready_events = 0u32;
                if status.read && interest.contains(PollEvents::IN) {
                    ready_events |= PollEvents::IN.bits() as u32;
                }
                if status.write && interest.contains(PollEvents::OUT) {
                    ready_events |= PollEvents::OUT.bits() as u32;
                }
                if status.error {
                    ready_events |= PollEvents::ERR.bits() as u32;
                }

                if ready_events != 0 {
                    events.push(EpollEvent {
                        events: ready_events,
                        data: event.data,
                    });
                    if events.len() >= maxevents {
                        break;
                    }
                }
            }

            if !events.is_empty() {
                return Ok(events);
            }

            if timeout_msecs >= 0 {
                let deadline = begin_time + core::time::Duration::from_millis(timeout_msecs as u64);
                if kernel_hal::timer::timer_now() >= deadline {
                    return Ok(Vec::new());
                }
            }

            // Park a readiness waker on every watched fd that can carry one, so
            // a pipe write / unix-socket send / timerfd expiry wakes this task
            // the moment it happens instead of on the next re-scan tick. The
            // subscriptions are flat, synchronous registrations (see
            // `FileLike::subscribe_readiness` — NOT nested async_poll futures,
            // which overflowed the coroutine stack at desktop start). They are
            // taken AFTER the scan above found nothing: an event racing in
            // between is caught by the EventBus's latched flags, which fire the
            // waker at subscribe time.
            //
            // When every fd subscribed, the fallback re-scan stretches from
            // 4 ms to the covered tick — that alone removes ~96 % of the idle
            // wakeups of a parked desktop process. Any fd without an event
            // source (evdev nodes, signalfd, nested epolls) keeps the short
            // tick for its whole set, preserving the old behavior exactly.
            let waker =
                core::future::poll_fn(|cx| core::task::Poll::Ready(cx.waker().clone())).await;
            let mut subs = alloc::vec::Vec::with_capacity(interest_list.len());
            let mut covered = !interest_list.is_empty();
            for (_fd, event, file) in &interest_list {
                let interest = PollEvents::from_bits_truncate(event.events as u16);
                match file.subscribe_readiness(interest, &waker) {
                    Some(sub) => subs.push(sub),
                    None => covered = false,
                }
            }
            let tick_ms = if covered {
                crate::net::wait::IO_WAIT_COVERED_TICK_MS
            } else {
                crate::net::wait::IO_WAIT_TICK_MS
            };
            crate::net::wait::IoMultiplexWait::with_tick(
                timeout_msecs,
                watch_net,
                watch_interactive,
                tick_ms,
            )
            .await;
            drop(subs);
        }
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod abi_tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn epoll_event_matches_linux_uapi() {
        assert_eq!(size_of::<EpollEvent>(), 12);
    }
}

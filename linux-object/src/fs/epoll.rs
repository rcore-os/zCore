use super::*;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::future::Future;
use lock::Mutex;
use zircon_object::object::*;

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

    /// Returns whether any watched fd is currently ready for its requested
    /// events. Shared by `poll`/`async_poll` so a nested epoll surfaces its
    /// inner readiness to an outer epoll/poll.
    fn any_ready(&self) -> bool {
        // Snapshot the handles so we don't hold the lock across `poll()` calls
        // (a watched fd could itself be an epoll that re-enters).
        let entries: Vec<(EpollEvent, Arc<dyn FileLike>)> =
            self.inner.lock().interest_list.values().cloned().collect();
        for (event, file) in entries {
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
            // Scan readiness through `async_poll` with this task's own Context:
            // files with real wakers (pipes, unix sockets, …) park one on their
            // eventbus while Pending, so the write that makes an fd ready wakes
            // this epoll immediately instead of waiting for the fallback tick
            // below. Files whose async_poll reports a status synchronously
            // behave exactly as the old sync `poll()` scan did.
            let events = core::future::poll_fn(|cx| {
                let mut events = Vec::new();
                for (_fd, event, file) in &interest_list {
                    let interest = PollEvents::from_bits_truncate(event.events as u16);
                    let mut fut = alloc::boxed::Box::pin(file.async_poll(interest));
                    let status = match fut.as_mut().poll(cx) {
                        core::task::Poll::Ready(Ok(status)) => status,
                        core::task::Poll::Ready(Err(err)) => {
                            return core::task::Poll::Ready(Err(err))
                        }
                        // Not ready; a waker is now parked on the file.
                        core::task::Poll::Pending => continue,
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
                core::task::Poll::Ready(Ok(events))
            })
            .await?;

            if !events.is_empty() {
                return Ok(events);
            }

            if timeout_msecs >= 0 {
                let deadline = begin_time + core::time::Duration::from_millis(timeout_msecs as u64);
                if kernel_hal::timer::timer_now() >= deadline {
                    return Ok(Vec::new());
                }
            }

            crate::net::wait::IoMultiplexWait::new(timeout_msecs, watch_net, watch_interactive)
                .await;
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

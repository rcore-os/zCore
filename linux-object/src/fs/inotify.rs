//! Minimal `inotify(7)` implementation.
//!
//! labwc (and many GTK apps) create an inotify instance at startup to watch
//! their config directory for live-reload. This kernel had no inotify, so the
//! call returned `unknown syscall: INOTIFY_INIT1` (ENOSYS) — on a real-hardware
//! bring-up that aborted labwc's config-watcher setup before the compositor
//! ever came up.
//!
//! This is a *functional stub*: `inotify_init1` returns a real, pollable fd and
//! `inotify_add_watch`/`inotify_rm_watch` hand out and track watch descriptors,
//! but no filesystem here delivers change events, so the fd simply never
//! becomes readable. That is exactly the "no events" state a quiescent inotify
//! fd is already allowed to be in — the watcher exists and polls cleanly, the
//! client runs, and config hot-reload is silently disabled rather than fatal.

use super::*;
use crate::sync::{Event, EventBus};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use lock::Mutex;
use zircon_object::object::*;

/// inotify instance backing an `inotify_init1(2)` fd.
pub struct Inotify {
    base: KObjectBase,
    inner: Arc<Mutex<InotifyInner>>,
    eventbus: Arc<Mutex<EventBus>>,
    flags: OpenFlags,
}

#[derive(Default)]
struct InotifyInner {
    /// Next watch descriptor to hand out. Linux watch descriptors are small
    /// positive ints, unique per inotify instance and monotonically assigned.
    next_wd: i32,
    /// wd -> (pathname, mask). Kept so `inotify_add_watch` on an already
    /// watched path returns the SAME wd (Linux semantics) and `inotify_rm_watch`
    /// can validate the descriptor.
    watches: BTreeMap<i32, (String, u32)>,
}

impl_kobject!(Inotify);

impl Inotify {
    /// Create an inotify instance. `flags` carries `IN_NONBLOCK`/`IN_CLOEXEC`,
    /// which share the `O_NONBLOCK`/`O_CLOEXEC` bit values.
    pub fn new(flags: OpenFlags) -> Arc<Self> {
        Arc::new(Inotify {
            base: KObjectBase::new(),
            inner: Arc::new(Mutex::new(InotifyInner {
                next_wd: 1,
                watches: BTreeMap::new(),
            })),
            eventbus: EventBus::new(),
            flags,
        })
    }

    /// Add or update a watch. Returns the watch descriptor. Re-watching an
    /// existing path returns its existing wd with the mask merged/replaced,
    /// matching `inotify_add_watch(2)`.
    pub fn add_watch(&self, path: &str, mask: u32) -> LxResult<usize> {
        let mut inner = self.inner.lock();
        if let Some((&wd, _)) = inner.watches.iter().find(|(_, (p, _))| p == path) {
            inner.watches.insert(wd, (path.into(), mask));
            return Ok(wd as usize);
        }
        let wd = inner.next_wd;
        inner.next_wd += 1;
        inner.watches.insert(wd, (path.into(), mask));
        Ok(wd as usize)
    }

    /// Remove a watch by descriptor. `EINVAL` if it is not a live descriptor,
    /// as Linux does.
    pub fn rm_watch(&self, wd: i32) -> LxResult<usize> {
        let mut inner = self.inner.lock();
        if inner.watches.remove(&wd).is_some() {
            Ok(0)
        } else {
            Err(LxError::EINVAL)
        }
    }
}

#[async_trait]
impl FileLike for Inotify {
    fn flags(&self) -> OpenFlags {
        self.flags
    }

    fn set_flags(&self, _f: OpenFlags) -> LxResult {
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            inner: self.inner.clone(),
            eventbus: self.eventbus.clone(),
            flags: self.flags,
        })
    }

    async fn read(&self, _buf: &mut [u8]) -> LxResult<usize> {
        // No events are ever generated. A non-blocking reader gets EAGAIN
        // (the normal "nothing pending" answer); a blocking reader parks on
        // the eventbus that never fires — exactly how a real inotify fd with
        // no pending events behaves.
        if self.flags.contains(OpenFlags::NON_BLOCK) {
            return Err(LxError::EAGAIN);
        }
        let bus = self.eventbus.clone();
        crate::sync::wait_for_event(bus, Event::READABLE).await;
        Err(LxError::EAGAIN)
    }

    fn write(&self, _buf: &[u8]) -> LxResult<usize> {
        // inotify fds are read-only.
        Err(LxError::EINVAL)
    }

    async fn read_at(&self, _offset: u64, buf: &mut [u8]) -> LxResult<usize> {
        self.read(buf).await
    }

    fn poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        // Never readable (no events), never writable (read-only), no error.
        Ok(PollStatus {
            read: false,
            write: false,
            error: false,
        })
    }

    async fn async_poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        // Park until an event that never comes; epoll/poll treat this fd as
        // simply not ready. Returning the synchronous status immediately would
        // spin a caller that only cares about POLLIN, so wait on the (silent)
        // eventbus like the blocking read does.
        let bus = self.eventbus.clone();
        crate::sync::wait_for_event(bus, Event::READABLE).await;
        self.poll(_events)
    }
}

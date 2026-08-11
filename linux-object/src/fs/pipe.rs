//! Implement INode for Pipe
#![deny(missing_docs)]

use crate::{sync::Event, sync::EventBus};
use alloc::{boxed::Box, collections::vec_deque::VecDeque, sync::Arc};
use core::{any::Any, cmp::min};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use lock::Mutex;
use rcore_fs::vfs::*;

#[derive(Clone, PartialEq, Eq)]
#[allow(dead_code)]
/// Pipe end specify
pub enum PipeEnd {
    /// read end
    Read,
    /// write end
    Write,
}

/// Default pipe capacity reported by `fcntl(F_GETPIPE_SZ)`: 16 pages, the
/// Linux default since 2.6.11.
pub const PIPE_DEFAULT_CAPACITY: usize = 65536;

/// Pipe inner data
pub struct PipeData {
    /// pipe buffer
    buf: VecDeque<u8>,
    /// event bus for pipe
    eventbus: EventBus,
    /// number of read ends
    read_cnt: i32,
    /// number of write ends
    write_cnt: i32,
    /// Nominal capacity for `F_GETPIPE_SZ`/`F_SETPIPE_SZ` round-trips. The
    /// byte queue itself is unbounded and writes never block on it — this
    /// value is what programs that tune their pipes (`pv`, shells sizing
    /// splice batches) get to read back.
    capacity: usize,
}

/// pipe struct
pub struct Pipe {
    data: Arc<Mutex<PipeData>>,
    direction: PipeEnd,
}

impl Clone for Pipe {
    fn clone(&self) -> Self {
        let mut data = self.data.lock();
        match self.direction {
            PipeEnd::Read => data.read_cnt += 1,
            PipeEnd::Write => data.write_cnt += 1,
        }
        Pipe {
            data: self.data.clone(),
            direction: self.direction.clone(),
        }
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // pipe end closed
        let mut data = self.data.lock();
        match self.direction {
            PipeEnd::Read => data.read_cnt -= 1,
            PipeEnd::Write => data.write_cnt -= 1,
        }
        // Latch CLOSED only when a whole SIDE is gone (EOF for readers /
        // EPIPE for writers) — that is when `poll()` starts reporting the
        // condition, so "CLOSED latched ⇒ a poll scan is ready" holds and a
        // parked readiness subscription can never immediate-fire in a loop.
        // The previous unconditional `set` latched CLOSED whenever ANY handle
        // dropped — including a dup'd fd with live siblings (routine in shell
        // redirections), which left the flag permanently on for a perfectly
        // healthy pipe and woke every later event-bus waiter spuriously.
        if data.read_cnt == 0 || data.write_cnt == 0 {
            data.eventbus.set(Event::CLOSED);
        }
    }
}

#[allow(dead_code)]
impl Pipe {
    /// Create a pair of INode: (read, write)
    pub fn create_pair() -> (Pipe, Pipe) {
        let inner = PipeData {
            buf: VecDeque::new(),
            eventbus: EventBus::default(),
            read_cnt: 1,
            write_cnt: 1,
            capacity: PIPE_DEFAULT_CAPACITY,
        };
        let data = Arc::new(Mutex::new(inner));
        (
            Pipe {
                data: data.clone(),
                direction: PipeEnd::Read,
            },
            Pipe {
                data,
                direction: PipeEnd::Write,
            },
        )
    }
    /// True when this handle is the read end of the pipe.
    pub fn is_read_end(&self) -> bool {
        self.direction == PipeEnd::Read
    }

    /// Park `waker` on this pipe's event bus for the next readiness
    /// transition (see `FileLike::subscribe_readiness`; reached through
    /// `File`'s inode downcast). The bus lives inside `PipeData`, so the
    /// unsubscribe handle captures the shared `Arc` and relocks it on drop.
    pub fn subscribe_readiness(
        &self,
        events: crate::fs::PollEvents,
        waker: &core::task::Waker,
    ) -> crate::sync::ReadinessSub {
        let mask = crate::fs::poll_events_to_bus_mask(events);
        let id = {
            let mut data = self.data.lock();
            crate::sync::subscribe_waker(&mut data.eventbus, mask, waker)
        };
        match id {
            Some(id) => {
                let data = self.data.clone();
                crate::sync::ReadinessSub::new(Box::new(move || {
                    data.lock().eventbus.unsubscribe(id);
                }))
            }
            None => crate::sync::ReadinessSub::noop(),
        }
    }

    /// Nominal capacity, as reported by `fcntl(F_GETPIPE_SZ)`. Shared between
    /// both ends, like the kernel's pipe buffer is.
    pub fn capacity(&self) -> usize {
        self.data.lock().capacity
    }

    /// Set the nominal capacity (`fcntl(F_SETPIPE_SZ)`); the caller has
    /// already rounded and bounds-checked the value.
    pub fn set_capacity(&self, cap: usize) {
        self.data.lock().capacity = cap;
    }

    /// Copy up to `len` buffered bytes without consuming them (`tee(2)`), plus
    /// whether any write end is still open — which is what distinguishes
    /// "would block" from end-of-stream when the buffer comes back empty.
    /// `None` when called on the write end.
    pub fn peek_data(&self, len: usize) -> Option<(alloc::vec::Vec<u8>, bool)> {
        if self.direction != PipeEnd::Read {
            return None;
        }
        let data = self.data.lock();
        let out = data.buf.iter().take(len).copied().collect();
        Some((out, data.write_cnt > 0))
    }

    /// whether the pipe struct is readable
    fn can_read(&self) -> bool {
        if let PipeEnd::Read = self.direction {
            // true
            let data = self.data.lock();
            !data.buf.is_empty() || data.write_cnt == 0 // other end closed
        } else {
            false
        }
    }

    /// whether the pipe struct is writeable
    fn can_write(&self) -> bool {
        if let PipeEnd::Write = self.direction {
            self.data.lock().read_cnt > 0
        } else {
            false
        }
    }
}

impl INode for Pipe {
    /// read from pipe
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if let PipeEnd::Read = self.direction {
            let mut data = self.data.lock();
            if data.buf.is_empty() && data.write_cnt > 0 {
                Err(FsError::Again)
            } else {
                let len = min(buf.len(), data.buf.len());
                for item in buf.iter_mut().take(len) {
                    *item = data.buf.pop_front().unwrap();
                }
                if data.buf.is_empty() {
                    data.eventbus.clear(Event::READABLE);
                }
                Ok(len)
            }
        } else {
            Ok(0)
        }
    }

    /// write to pipe
    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        if let PipeEnd::Write = self.direction {
            let mut data = self.data.lock();
            for c in buf {
                data.buf.push_back(*c);
            }
            data.eventbus.set(Event::READABLE);
            Ok(buf.len())
        } else {
            Ok(0)
        }
    }

    /// monitoring events and determine whether the pipe is readable or writeable
    /// if the write end is not close and the buffer is empty, the read end will be block
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: self.can_read(),
            write: self.can_write(),
            error: false,
        })
    }

    fn async_poll<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<PollStatus>> + Send + Sync + 'a>> {
        #[must_use = "future does nothing unless polled/`await`-ed"]
        struct PipeFuture<'a> {
            pipe: &'a Pipe,
            sub_id: Option<u64>,
        }

        impl Drop for PipeFuture<'_> {
            fn drop(&mut self) {
                if let Some(id) = self.sub_id.take() {
                    self.pipe.data.lock().eventbus.unsubscribe(id);
                }
            }
        }

        impl<'a> Future for PipeFuture<'a> {
            type Output = Result<PollStatus>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                // Readiness and subscription under ONE hold of the pipe lock.
                // The previous shape checked readiness (taking and releasing
                // the lock inside `can_read`/`can_write`), and only then
                // re-took the lock to subscribe — a window in which the peer
                // could push its byte and fire the eventbus at nobody. With the
                // bus latching flags and firing only on transitions, a wake
                // missed there was not late, it was lost for good. The
                // subscribe-time check in `EventBus::subscribe` now also closes
                // this generically; doing it under one lock here means the pipe
                // does not depend on that second line of defense.
                //
                // sub_id + Drop: poll/epoll re-scan drops this future every
                // pass; without unsubscribe, orphaned wakers pile up and a
                // later pipe write wakes a freed task (UAF → delayed PAGE FAULT).
                let this = self.get_mut();
                let mut data = this.pipe.data.lock();
                let ready = match this.pipe.direction {
                    PipeEnd::Read => !data.buf.is_empty() || data.write_cnt == 0,
                    PipeEnd::Write => data.read_cnt > 0,
                };
                if ready {
                    if let Some(id) = this.sub_id.take() {
                        data.eventbus.unsubscribe(id);
                    }
                    drop(data);
                    return Poll::Ready(this.pipe.poll());
                }
                if this.sub_id.is_none() {
                    let waker = cx.waker().clone();
                    this.sub_id = data.eventbus.subscribe(Box::new({
                        move |_| {
                            waker.wake_by_ref();
                            true
                        }
                    }));
                }
                Poll::Pending
            }
        }

        Box::pin(PipeFuture {
            pipe: self,
            sub_id: None,
        })
    }

    /// return the any ref
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, Ordering};
    use core::task::{RawWaker, RawWakerVTable, Waker};

    fn flag_waker(flag: &'static AtomicBool) -> Waker {
        fn raw(ptr: *const ()) -> RawWaker {
            unsafe fn clone(ptr: *const ()) -> RawWaker {
                raw(ptr)
            }
            unsafe fn wake(ptr: *const ()) {
                (*(ptr as *const AtomicBool)).store(true, Ordering::SeqCst);
            }
            unsafe fn wake_by_ref(ptr: *const ()) {
                (*(ptr as *const AtomicBool)).store(true, Ordering::SeqCst);
            }
            unsafe fn drop(_: *const ()) {}
            RawWaker::new(ptr, &RawWakerVTable::new(clone, wake, wake_by_ref, drop))
        }
        unsafe { Waker::from_raw(raw(flag as *const AtomicBool as *const ())) }
    }

    /// poll/epoll drops Pending async_poll futures every re-scan; without
    /// unsubscribe that leaks wakers into UAF when the pipe later gets data.
    #[test]
    fn async_poll_drop_unsubscribes() {
        static WOKE: AtomicBool = AtomicBool::new(false);
        WOKE.store(false, Ordering::SeqCst);

        let (r, _w) = Pipe::create_pair();
        // Empty read end with a live writer → not ready → must subscribe.
        assert!(!r.can_read());

        let waker = flag_waker(&WOKE);
        let mut cx = Context::from_waker(&waker);
        let mut fut = r.async_poll();
        assert!(matches!(fut.as_mut().poll(&mut cx), Poll::Pending));
        let n = r.data.lock().eventbus.get_callback_len();
        assert!(n >= 1, "Pending poll must park a waker");

        drop(fut);
        assert_eq!(
            r.data.lock().eventbus.get_callback_len(),
            n - 1,
            "Drop must unsubscribe the parked pipe waker"
        );
        assert!(!WOKE.load(Ordering::SeqCst));
    }

    /// Compressed stand-in for a 30–40 min compositor session: tens of thousands
    /// of poll re-scans must leave the pipe EventBus empty.
    #[test]
    fn async_poll_drop_soak_does_not_fill_bus() {
        static WOKE: AtomicBool = AtomicBool::new(false);
        WOKE.store(false, Ordering::SeqCst);

        let (r, _w) = Pipe::create_pair();
        let waker = flag_waker(&WOKE);
        let mut cx = Context::from_waker(&waker);

        for _ in 0..20_000 {
            let mut fut = r.async_poll();
            assert!(matches!(fut.as_mut().poll(&mut cx), Poll::Pending));
            drop(fut);
            assert_eq!(
                r.data.lock().eventbus.get_callback_len(),
                0,
                "each Drop must clear the parked pipe waker"
            );
        }
    }
}

//! `signalfd(2)` — accept signals through a readable file descriptor instead of
//! a handler. libwayland's `wl_event_loop_add_signal` blocks a signal and reads
//! it here, so a Wayland compositor (labwc) handles SIGINT/SIGTERM/SIGCHLD from
//! its event loop. Without it, the blocked signal sits pending forever and
//! Ctrl-C does nothing.

use super::*;
// `crate::signal::Signal` (the Linux signal enum) would shadow
// `zircon_object::object::Signal` (the KObject signal bits used by
// `impl_kobject!`), so alias it.
use crate::signal::{Signal as LinuxSignal, Sigset};
use crate::thread::ThreadExt;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};
use zircon_object::object::*;
use zircon_object::task::Thread;

/// `sizeof(struct signalfd_siginfo)`.
const SIGINFO_SIZE: usize = 128;

/// signalfd implementation. The set of accepted signals is mutable
/// (`signalfd4` with an existing fd updates it).
pub struct SignalFd {
    base: KObjectBase,
    mask: Arc<AtomicU64>,
    flags: OpenFlags,
}

impl_kobject!(SignalFd);

impl SignalFd {
    /// Create a signalfd watching the signals in `mask`.
    pub fn new(mask: u64, flags: OpenFlags) -> Arc<Self> {
        Arc::new(SignalFd {
            base: KObjectBase::new(),
            mask: Arc::new(AtomicU64::new(mask)),
            flags,
        })
    }

    /// Replace the accepted-signal mask (`signalfd4` on an existing fd).
    pub fn set_mask(&self, mask: u64) {
        self.mask.store(mask, SeqCst);
    }

    /// The calling thread's pending signals that this fd accepts.
    fn pending_matched(&self) -> Sigset {
        let mask = self.mask.load(SeqCst);
        if let Some(arc) = kernel_hal::thread::get_current_thread() {
            if let Ok(thread) = arc.downcast::<Thread>() {
                let tl = thread.lock_linux();
                return Sigset::new(tl.signals.val() & mask);
            }
        }
        Sigset::empty()
    }

    /// Consume and return the lowest-numbered accepted pending signal, removing
    /// it from the calling thread's pending set.
    fn consume_one(&self) -> Option<LinuxSignal> {
        let mask = self.mask.load(SeqCst);
        let arc = kernel_hal::thread::get_current_thread()?;
        let thread = arc.downcast::<Thread>().ok()?;
        let mut tl = thread.lock_linux();
        let sig = Sigset::new(tl.signals.val() & mask).find_first_signal()?;
        tl.signals.remove(sig);
        Some(sig)
    }
}

#[async_trait]
impl FileLike for SignalFd {
    fn flags(&self) -> OpenFlags {
        self.flags
    }

    fn set_flags(&self, _f: OpenFlags) -> LxResult {
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            mask: self.mask.clone(),
            flags: self.flags,
        })
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        if buf.len() < SIGINFO_SIZE {
            return Err(LxError::EINVAL);
        }
        loop {
            if let Some(sig) = self.consume_one() {
                // struct signalfd_siginfo: ssi_signo is the first u32; the rest
                // (errno/code/pid/uid/…) we leave zero, which is all an event
                // loop reading Ctrl-C / SIGTERM looks at.
                buf[..SIGINFO_SIZE].fill(0);
                buf[..4].copy_from_slice(&(sig as u32).to_ne_bytes());
                return Ok(SIGINFO_SIZE);
            }
            if self.flags.contains(OpenFlags::NON_BLOCK) {
                return Err(LxError::EAGAIN);
            }
            // Block until a matching signal is pending. Signals don't fire a
            // per-fd waker, so re-check on a short timer rather than spinning.
            // The realistic user (libwayland) polls via epoll and never reaches
            // this path; epoll's own re-poll tick bounds its latency.
            let deadline = kernel_hal::timer::timer_now() + core::time::Duration::from_millis(20);
            kernel_hal::thread::sleep_until(deadline).await;
        }
    }

    fn write(&self, _buf: &[u8]) -> LxResult<usize> {
        Err(LxError::EINVAL)
    }

    async fn read_at(&self, _offset: u64, buf: &mut [u8]) -> LxResult<usize> {
        self.read(buf).await
    }

    fn poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        Ok(PollStatus {
            read: self.pending_matched().is_not_empty(),
            write: false,
            error: false,
        })
    }

    async fn async_poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        // Signals don't fire an EventBus, so this future resolves only once a
        // matching signal is already pending; callers (epoll/select) re-poll on
        // their own timer tick, which bounds the latency.
        let status = self.poll(_events)?;
        Ok(status)
    }
}

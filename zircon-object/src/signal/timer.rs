use super::*;
use crate::object::*;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::time::Duration;
use spin::Mutex;

/// An object that may be signaled at some point in the future
///
/// ## SYNOPSIS
///
/// A timer is used to wait until a specified point in time has occurred
/// or the timer has been canceled.
pub struct Timer {
    base: KObjectBase,
    inner: Mutex<TimerInner>,
}

impl_kobject!(Timer);

#[derive(Default)]
struct TimerInner {
    deadline: Option<Duration>,
}

impl Timer {
    /// Create a new `Timer`.
    pub fn new() -> Arc<Self> {
        Arc::new(Timer {
            base: {
                let mut res = KObjectBase::default();
                res.obj_type = OBJ_TYPE_TIMER;
                res
            },
            inner: Mutex::default(),
        })
    }

    /// Starts a one-shot timer that will fire when `deadline` passes.
    ///
    /// If a previous call to `set` was pending, the previous timer is canceled
    /// and `Signal::SIGNALED` is de-asserted as needed.
    pub fn set(self: &Arc<Self>, deadline: Duration, _slack: Duration) {
        let mut inner = self.inner.lock();
        inner.deadline = Some(deadline);
        self.base.signal_clear(Signal::SIGNALED);
        let me = self.clone();
        kernel_hal::timer_set(deadline, Box::new(move |now| me.touch(now)));
    }

    /// Cancel the pending timer started by `set`.
    pub fn cancel(&self) {
        let mut inner = self.inner.lock();
        inner.deadline = None;
    }

    /// Called by HAL timer.
    fn touch(&self, now: Duration) {
        let mut inner = self.inner.lock();
        if let Some(deadline) = inner.deadline {
            if now >= deadline {
                self.base.signal_set(Signal::SIGNALED);
                inner.deadline = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_hal::timer_now;

    #[test]
    fn set() {
        let timer = Timer::new();
        timer.set(timer_now() + Duration::from_millis(10), Duration::default());
        timer.set(timer_now() + Duration::from_millis(20), Duration::default());

        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(timer.signal(), Signal::empty());

        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(timer.signal(), Signal::SIGNALED);

        timer.set(timer_now() + Duration::from_millis(10), Duration::default());
        assert_eq!(timer.signal(), Signal::empty());
    }

    #[test]
    fn cancel() {
        let timer = Timer::new();
        timer.set(timer_now() + Duration::from_millis(10), Duration::default());

        std::thread::sleep(Duration::from_millis(5));
        timer.cancel();

        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(timer.signal(), Signal::empty());
    }
}

use super::*;
use crate::object::*;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};
use spin::Mutex;

const SLACK_CENTER: u32 = 0;
const SLACK_EARLY: u32 = 1;
const SLACK_LATE: u32 = 2;

/// An object that may be signaled at some point in the future
///
/// ## SYNOPSIS
///
/// A timer is used to wait until a specified point in time has occurred
/// or the timer has been canceled.
pub struct Timer {
    base: KObjectBase,
    #[allow(dead_code)]
    flags: u32,
    inner: Mutex<TimerInner>,
}

impl_kobject!(Timer);

#[derive(Default)]
struct TimerInner {
    deadline: Option<Duration>,
}

impl Timer {
    /// Create a new `Timer`.
    pub fn create(flags: u32) -> ZxResult<Arc<Self>> {
        match flags {
            SLACK_LATE | SLACK_EARLY | SLACK_CENTER => Ok(Arc::new(Timer {
                base: KObjectBase::default(),
                flags,
                inner: Mutex::default(),
            })),
            _ => Err(ZxError::INVALID_ARGS),
        }
    }

    /// Starts a one-shot timer that will fire when `deadline` passes.
    ///
    /// If a previous call to `set` was pending, the previous timer is canceled
    /// and `Signal::SIGNALED` is de-asserted as needed.
    pub fn set(self: &Arc<Self>, deadline: Duration, _slack: Duration) {
        let mut inner = self.inner.lock();
        inner.deadline = Some(deadline);
        self.base.signal_clear(Signal::SIGNALED);
        let me = Arc::downgrade(self);
        kernel_hal::timer_set(
            deadline,
            Box::new(move |now| me.upgrade().map(|timer| timer.touch(now)).unwrap_or(())),
        );
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

#[derive(Default)]
pub struct YieldFutureImpl {
    flag: bool,
}

impl Future for YieldFutureImpl {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if self.flag {
            Poll::Ready(())
        } else {
            self.flag = true;
            cx.waker().clone().wake();
            Poll::Pending
        }
    }
}

pub struct SleepState {
    timer: Arc<Timer>,
    inner: Mutex<SleepStateInner>,
}

impl SleepState {
    pub fn new() -> Arc<Self> {
        Arc::new(SleepState {
            timer: Timer::create(0).unwrap(),
            inner: Mutex::new(SleepStateInner::default()),
        })
    }

    pub fn set_deadline(self: &Arc<SleepState>, deadline: Duration) {
        self.timer.set(deadline, Duration::from_nanos(0));
        let weak_self = Arc::downgrade(self);
        self.timer.add_signal_callback(Box::new(move |signal| {
            if let Some(real_self) = weak_self.upgrade() {
                assert!(!(signal & Signal::SIGNALED).is_empty());
                let mut inner = real_self.inner.lock();
                inner.woken = true;
                inner.waker.as_ref().unwrap().wake_by_ref();
                true
            } else {
                true
            }
        }));
    }

    fn is_woken(&self) -> bool {
        self.inner.lock().woken
    }

    fn renew_waker(&self, new_waker: Waker) {
        self.inner.lock().waker.replace(new_waker);
    }
}

pub struct SleepFutureImpl {
    pub state: Arc<SleepState>,
}

impl Future for SleepFutureImpl {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if self.state.is_woken() {
            Poll::Ready(())
        } else {
            self.state.renew_waker(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[derive(Default)]
struct SleepStateInner {
    waker: Option<Waker>,
    woken: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_hal::timer_now;

    #[test]
    fn set() {
        let timer = Timer::create(0).unwrap();
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
        let timer = Timer::create(0).unwrap();
        timer.set(timer_now() + Duration::from_millis(10), Duration::default());

        std::thread::sleep(Duration::from_millis(5));
        timer.cancel();

        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(timer.signal(), Signal::empty());
    }
}

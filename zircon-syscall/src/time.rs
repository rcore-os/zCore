use {
    super::*,
    core::{
        fmt::{Debug, Formatter, Result},
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    },
    kernel_hal::timer::timer_now,
    zircon_object::{dev::*, task::*},
};

static UTC_OFFSET: AtomicU64 = AtomicU64::new(0);

const ZX_CLOCK_MONOTONIC: u32 = 0;
const ZX_CLOCK_UTC: u32 = 1;
const ZX_CLOCK_THREAD: u32 = 2;

impl Syscall<'_> {
    /// Create a new clock object.
    pub fn sys_clock_create(
        &self,
        _options: u64,
        _user_args: UserInPtr<u8>,
        mut _out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        warn!("clock.create: skip");
        Ok(())
    }

    /// Acquire the current time.
    ///
    /// + Returns the current time of clock_id via `time`.
    /// + Returns whether `clock_id` was valid.
    pub fn sys_clock_get(&self, clock_id: u32, mut time: UserOutPtr<u64>) -> ZxResult {
        info!("clock.get: id={}", clock_id);
        match clock_id {
            ZX_CLOCK_MONOTONIC => {
                time.write(timer_now().as_nanos() as u64)?;
                Ok(())
            }
            ZX_CLOCK_UTC => {
                time.write(timer_now().as_nanos() as u64 + UTC_OFFSET.load(Ordering::Relaxed))?;
                Ok(())
            }
            ZX_CLOCK_THREAD => {
                time.write(self.thread.get_time())?;
                Ok(())
            }
            _ => Err(ZxError::NOT_SUPPORTED),
        }
    }

    /// Perform a basic read of the clock.
    pub fn sys_clock_read(&self, handle: HandleValue, mut now: UserOutPtr<u64>) -> ZxResult {
        info!("clock.read: handle={:#x?}", handle);
        warn!("ignore clock handle");
        now.write(timer_now().as_nanos() as u64)?;
        Ok(())
    }

    ///
    pub fn sys_clock_adjust(&self, resource: HandleValue, clock_id: u32, offset: u64) -> ZxResult {
        info!(
            "clock.adjust: resource={:#x?}, id={:#x}, offset={:#x}",
            resource, clock_id, offset
        );
        let proc = self.thread.proc();
        proc.get_object::<Resource>(resource)?
            .validate(ResourceKind::ROOT)?;
        match clock_id {
            ZX_CLOCK_MONOTONIC => Err(ZxError::ACCESS_DENIED),
            ZX_CLOCK_UTC => {
                UTC_OFFSET.store(offset, Ordering::Relaxed);
                Ok(())
            }
            _ => Err(ZxError::INVALID_ARGS),
        }
    }

    /// Make adjustments to a clock object.
    pub fn sys_clock_update(
        &self,
        _handle: HandleValue,
        _options: u64,
        _user_args: UserInPtr<u8>,
    ) -> ZxResult {
        warn!("clock.update: skip");
        Ok(())
    }

    /// Sleep for some number of nanoseconds.
    ///
    /// A `deadline` value less than or equal to 0 immediately yields the thread.
    pub async fn sys_nanosleep(&self, deadline: Deadline) -> ZxResult {
        info!("nanosleep: deadline={:?}", deadline);
        if deadline.0 <= 0 {
            kernel_hal::thread::yield_now().await;
        } else {
            let future = kernel_hal::thread::sleep_until(deadline.into());
            pin_mut!(future);
            self.thread
                .blocking_run(
                    future,
                    ThreadState::BlockedSleeping,
                    Deadline::forever().into(),
                    None,
                )
                .await?;
        }
        Ok(())
    }
}

#[repr(transparent)]
pub struct Deadline(i64);

impl From<usize> for Deadline {
    fn from(x: usize) -> Self {
        Deadline(x as i64)
    }
}

impl Deadline {
    pub fn is_positive(&self) -> bool {
        self.0.is_positive()
    }

    pub fn forever() -> Self {
        Deadline(i64::max_value())
    }
}

impl From<Deadline> for Duration {
    fn from(deadline: Deadline) -> Self {
        Duration::from_nanos(deadline.0.max(0) as u64)
    }
}

impl Debug for Deadline {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        if self.0 <= 0 {
            write!(f, "NoWait")
        } else if self.0 == i64::max_value() {
            write!(f, "Forever")
        } else {
            write!(f, "At({:?})", Duration::from_nanos(self.0 as u64))
        }
    }
}

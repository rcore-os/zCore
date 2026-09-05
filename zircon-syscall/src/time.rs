use {
    super::*,
    core::{
        fmt::{Debug, Formatter, Result},
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    },
    kernel_hal::timer::timer_now,
    zircon_object::{dev::*, object::Clock, task::*},
};

static UTC_OFFSET: AtomicU64 = AtomicU64::new(0);

const ZX_CLOCK_MONOTONIC: u32 = 0;
const ZX_CLOCK_UTC: u32 = 1;
const ZX_CLOCK_THREAD: u32 = 2;

const ZX_CLOCK_ARGS_VERSION_SHIFT: u64 = 58;
const ZX_CLOCK_ARGS_VERSION_MASK: u64 = 0x3f << ZX_CLOCK_ARGS_VERSION_SHIFT;
const ZX_CLOCK_OPT_MONOTONIC: u64 = 1 << 0;
const ZX_CLOCK_OPT_CONTINUOUS: u64 = 1 << 1;
const ZX_CLOCK_OPT_AUTO_START: u64 = 1 << 2;
const ZX_CLOCK_OPT_BOOT: u64 = 1 << 3;
const ZX_CLOCK_OPT_MAPPABLE: u64 = 1 << 4;
const ZX_CLOCK_OPTS_ALL: u64 = ZX_CLOCK_OPT_MONOTONIC
    | ZX_CLOCK_OPT_CONTINUOUS
    | ZX_CLOCK_OPT_AUTO_START
    | ZX_CLOCK_OPT_BOOT
    | ZX_CLOCK_OPT_MAPPABLE;
const ZX_CLOCK_UPDATE_OPTION_SYNTHETIC_VALUE_VALID: u64 = 1 << 0;
const ZX_CLOCK_UPDATE_OPTION_REFERENCE_VALUE_VALID: u64 = 1 << 3;

#[repr(C)]
#[derive(Clone, Copy)]
struct ClockCreateArgsV1 {
    backstop_time: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClockUpdateArgsV1 {
    rate_adjust: i32,
    padding: [u8; 4],
    value: i64,
    error_bound: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClockUpdateArgsV2 {
    rate_adjust: i32,
    padding: [u8; 4],
    synthetic_value: i64,
    reference_value: i64,
    error_bound: u64,
}

impl Syscall<'_> {
    /// Create a new clock object.
    pub fn sys_clock_create(
        &self,
        options: u64,
        user_args: UserInPtr<u8>,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        let version = options >> ZX_CLOCK_ARGS_VERSION_SHIFT;
        if version > 1 || options & !(ZX_CLOCK_ARGS_VERSION_MASK | ZX_CLOCK_OPTS_ALL) != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let backstop = match version {
            0 => 0,
            1 => {
                UserInPtr::<ClockCreateArgsV1>::from(user_args.as_addr())
                    .read()?
                    .backstop_time
            }
            _ => unreachable!(),
        };
        let clock = Clock::new(backstop, options & ZX_CLOCK_OPT_MAPPABLE != 0);
        let handle = self
            .thread
            .proc()
            .add_handle(Handle::new(clock, Rights::DEFAULT_CLOCK));
        out.write(handle)?;
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
        let clock = self
            .thread
            .proc()
            .get_object_with_rights::<Clock>(handle, Rights::READ)?;
        now.write(clock.read() as u64)?;
        Ok(())
    }

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
        handle: HandleValue,
        options: u64,
        user_args: UserInPtr<u8>,
    ) -> ZxResult {
        let clock = self
            .thread
            .proc()
            .get_object_with_rights::<Clock>(handle, Rights::WRITE)?;
        let version = options >> ZX_CLOCK_ARGS_VERSION_SHIFT;
        let flags = options & !ZX_CLOCK_ARGS_VERSION_MASK;
        let now = timer_now().as_nanos() as i64;
        match version {
            1 => {
                let args = UserInPtr::<ClockUpdateArgsV1>::from(user_args.as_addr()).read()?;
                if flags & ZX_CLOCK_UPDATE_OPTION_SYNTHETIC_VALUE_VALID != 0 {
                    clock.update(now, args.value);
                }
            }
            2 => {
                let args = UserInPtr::<ClockUpdateArgsV2>::from(user_args.as_addr()).read()?;
                if flags & ZX_CLOCK_UPDATE_OPTION_SYNTHETIC_VALUE_VALID != 0 {
                    let reference = if flags & ZX_CLOCK_UPDATE_OPTION_REFERENCE_VALUE_VALID != 0 {
                        args.reference_value
                    } else {
                        now
                    };
                    clock.update(reference, args.synthetic_value);
                }
            }
            _ => return Err(ZxError::INVALID_ARGS),
        }
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
        Deadline(i64::MAX)
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
        } else if self.0 == i64::MAX {
            write!(f, "Forever")
        } else {
            write!(f, "At({:?})", Duration::from_nanos(self.0 as u64))
        }
    }
}

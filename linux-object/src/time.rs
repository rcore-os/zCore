//! Linux time objects

use alloc::sync::Arc;
use core::time::Duration;
use rcore_fs::vfs::*;

/// TimeSpec struct for clock_gettime, similar to Timespec
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct TimeSpec {
    /// seconds
    pub sec: usize,
    /// nano seconds
    pub nsec: usize,
}

/// TimeVal struct for gettimeofday
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct TimeVal {
    /// seconds
    pub sec: usize,
    /// microsecond
    pub usec: usize,
}

/// ITimerVal struct for setitimer/getitimer
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct ITimerVal {
    /// timer interval
    pub interval: TimeVal,
    /// current value
    pub value: TimeVal,
}

/// `struct itimerspec` for `timer_settime`/`timer_gettime` (nanosecond res).
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct ITimerSpec {
    /// timer period (0 = one-shot)
    pub interval: TimeSpec,
    /// time until next expiration
    pub value: TimeSpec,
}

impl From<TimeVal> for Duration {
    fn from(t: TimeVal) -> Self {
        Duration::from_secs(t.sec as u64) + Duration::from_micros(t.usec as u64)
    }
}

impl From<Duration> for TimeVal {
    fn from(d: Duration) -> Self {
        TimeVal {
            sec: d.as_secs() as usize,
            usec: d.subsec_micros() as usize,
        }
    }
}

/// Kernel-side state of one `setitimer(2)` slot
/// (ITIMER_REAL / ITIMER_VIRTUAL / ITIMER_PROF).
#[derive(Debug, Default, Clone, Copy)]
pub struct ItimerSlot {
    /// Reload period; zero means one-shot.
    pub interval: Duration,
    /// Absolute expiry on the boot-monotonic clock; `None` while disarmed.
    pub deadline: Option<Duration>,
    /// Bumped on every arm/disarm. A timer callback captures the value it was
    /// armed with and compares before firing, so a replaced or cancelled timer
    /// expires silently instead of delivering a stale signal.
    pub generation: u64,
}

impl TimeVal {
    /// create TimeVal
    pub fn now() -> TimeVal {
        TimeSpec::now().into()
    }
    /// Monotonic time since boot (`CLOCK_MONOTONIC`). Used to timestamp evdev
    /// input events: libinput selects `CLOCK_MONOTONIC` via `EVIOCSCLOCKID` and
    /// compares event times against `clock_gettime(CLOCK_MONOTONIC)`. Stamping
    /// events with the wall clock instead makes libinput's timers (button
    /// debounce, tap, scroll) see multi-second offsets and misbehave.
    pub fn now_monotonic() -> TimeVal {
        TimeSpec::now_monotonic().into()
    }
    /// to msec
    pub fn to_msec(&self) -> usize {
        self.sec * 1_000 + self.usec / 1_000
    }
}

impl TimeSpec {
    /// Build from a kernel `Duration` (seconds since Unix epoch for wall clock).
    pub fn from_duration(time: Duration) -> TimeSpec {
        TimeSpec {
            sec: time.as_secs() as usize,
            nsec: time.subsec_nanos() as usize,
        }
    }

    /// Wall-clock time (`CLOCK_REALTIME`, `gettimeofday`, `date`).
    pub fn now() -> TimeSpec {
        Self::from_duration(kernel_hal::timer::wall_clock_now())
    }

    /// Monotonic time since boot (`CLOCK_MONOTONIC`).
    pub fn now_monotonic() -> TimeSpec {
        Self::from_duration(kernel_hal::timer::timer_now())
    }

    /// update TimeSpec for a file inode
    /// TODO: more precise; update when write
    pub fn update(inode: &Arc<dyn INode>) {
        let now = TimeSpec::now().into();
        if let Ok(mut metadata) = inode.metadata() {
            metadata.atime = now;
            metadata.mtime = now;
            metadata.ctime = now;
            // silently fail for device file
            inode.set_metadata(&metadata).ok();
        }
    }

    /// to msec
    pub fn to_msec(&self) -> usize {
        self.sec * 1_000 + self.nsec / 1_000_000
    }
}

impl From<Timespec> for TimeSpec {
    fn from(t: Timespec) -> Self {
        Self {
            sec: t.sec as _,
            nsec: t.nsec as _,
        }
    }
}

impl From<TimeSpec> for Timespec {
    fn from(t: TimeSpec) -> Self {
        Self {
            sec: t.sec as _,
            nsec: t.nsec as _,
        }
    }
}

impl From<TimeSpec> for Duration {
    fn from(t: TimeSpec) -> Self {
        Self::new(t.sec as _, t.nsec as _)
    }
}

impl From<TimeSpec> for TimeVal {
    fn from(t: TimeSpec) -> Self {
        Self {
            sec: t.sec,
            usec: t.nsec / 1_000,
        }
    }
}

/// RUsage for sys_getrusage() — full Linux `struct rusage` layout.
///
/// Only the two time fields carry data; the 14 trailing longs
/// (`ru_maxrss` … `ru_nivcsw`) read as zero, which is also how Linux reports
/// the fields it does not maintain. Carrying them in the struct still matters:
/// when only the two timevals were written, the tail of the caller's buffer
/// kept whatever garbage was on the stack, and rusage consumers (`time(1)`,
/// libuv's getrusage wrapper) read uninitialized memory as huge fault counts.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct RUsage {
    /// user CPU time used
    pub utime: TimeVal,
    /// system CPU time used
    pub stime: TimeVal,
    /// ru_maxrss … ru_nivcsw, all reported as zero
    pub other: [i64; 14],
}

/// Tms for times()
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Tms {
    /// user time
    pub tms_utime: u64,
    /// system time
    pub tms_stime: u64,
    /// user time of children
    pub tms_cutime: u64,
    /// system time of children
    pub tms_cstime: u64,
}

/// Clock id
#[derive(Debug)]
#[repr(usize)]
pub enum ClockId {
    /// missing documentation
    ClockRealTime = 0,
    /// missing documentation
    ClockMonotonic = 1,
    /// missing documentation
    ClockProcessCpuTimeId = 2,
    /// missing documentation
    ClockThreadCpuTimeId = 3,
    /// missing documentation
    ClockMonotonicRaw = 4,
    /// missing documentation
    ClockRealTimeCoarse = 5,
    /// missing documentation
    ClockMonotonicCoarse = 6,
    /// missing documentation
    ClockBootTime = 7,
    /// missing documentation
    ClockRealTimeAlarm = 8,
    /// missing documentation
    ClockBootTimeAlarm = 9,
}

impl From<usize> for ClockId {
    fn from(t: usize) -> ClockId {
        match t {
            0 => ClockId::ClockRealTime,
            1 => ClockId::ClockMonotonic,
            2 => ClockId::ClockProcessCpuTimeId,
            3 => ClockId::ClockThreadCpuTimeId,
            4 => ClockId::ClockMonotonicRaw,
            5 => ClockId::ClockRealTimeCoarse,
            6 => ClockId::ClockMonotonicCoarse,
            7 => ClockId::ClockBootTime,
            8 => ClockId::ClockRealTimeAlarm,
            9 => ClockId::ClockBootTimeAlarm,
            _ => unreachable!(),
        }
    }
}

/// Clock Flags
#[derive(Debug)]
#[repr(usize)]
pub enum ClockFlags {
    /// missing documentation
    ZeroFlag = 0,
    /// missing documentation
    TimerAbsTime = 1,
}

impl From<usize> for ClockFlags {
    fn from(t: usize) -> ClockFlags {
        match t {
            0 => ClockFlags::ZeroFlag,
            1 => ClockFlags::TimerAbsTime,
            _ => unreachable!(),
        }
    }
}

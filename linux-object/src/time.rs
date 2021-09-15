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

impl TimeVal {
    /// create TimeVal
    pub fn now() -> TimeVal {
        TimeSpec::now().into()
    }
    /// to msec
    pub fn to_msec(&self) -> usize {
        self.sec * 1_000 + self.usec / 1_000
    }
}

impl TimeSpec {
    /// create TimeSpec
    pub fn now() -> TimeSpec {
        let time = kernel_hal::timer::timer_now();
        TimeSpec {
            sec: time.as_secs() as usize,
            nsec: (time.as_nanos() % 1_000_000_000) as usize,
        }
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

impl From<TimeSpec> for Timespec {
    fn from(t: TimeSpec) -> Self {
        Timespec {
            sec: t.sec as i64,
            nsec: t.nsec as i32,
        }
    }
}

impl From<TimeSpec> for Duration {
    fn from(t: TimeSpec) -> Self {
        Duration::new(t.sec as u64, t.nsec as u32)
    }
}

impl From<TimeSpec> for TimeVal {
    fn from(t: TimeSpec) -> Self {
        TimeVal {
            sec: t.sec,
            usec: t.nsec / 1_000,
        }
    }
}

/// RUsage for sys_getrusage()
/// ignore other fields for now
#[repr(C)]
pub struct RUsage {
    /// user CPU time used
    pub utime: TimeVal,
    /// system CPU time used
    pub stime: TimeVal,
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

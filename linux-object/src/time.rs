//! Linux time objects

use alloc::sync::Arc;
use core::time::Duration;
use kernel_hal::timer_now;
use rcore_fs::vfs::*;

/// TimeSpec struct for clock_gettime, similar to Timespec
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimeSpec {
    /// seconds
    pub sec: usize,
    /// nano seconds
    pub nsec: usize,
}

/// TimeVal struct for gettimeofday
#[repr(C)]
#[derive(Debug, Copy, Clone)]
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
}

impl TimeSpec {
    /// create TimeSpec
    pub fn now() -> TimeSpec {
        let time = timer_now();
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
}

impl Into<Timespec> for TimeSpec {
    fn into(self) -> Timespec {
        Timespec {
            sec: self.sec as i64,
            nsec: self.nsec as i32,
        }
    }
}

impl Into<Duration> for TimeSpec {
    fn into(self) -> Duration {
        Duration::new(self.sec as u64, self.nsec as u32)
    }
}

impl Into<TimeVal> for TimeSpec {
    fn into(self) -> TimeVal {
        TimeVal {
            sec: self.sec,
            usec: self.nsec / 1_000 as usize,
        }
    }
}

impl Default for TimeVal {
    fn default() -> Self {
        TimeVal { sec: 0, usec: 0 }
    }
}

impl Default for TimeSpec {
    fn default() -> Self {
        TimeSpec { sec: 0, nsec: 0 }
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

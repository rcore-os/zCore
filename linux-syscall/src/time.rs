//! Syscalls for time
//! - clock_gettime

const USEC_PER_TICK: usize = 10000;

use crate::Syscall;
use alloc::sync::Arc;
use core::time::Duration;
use kernel_hal::{timer_now, user::UserInPtr, user::UserOutPtr};
use linux_object::error::LxError;
use linux_object::error::SysResult;
use rcore_fs::vfs::*;

/// TimeSpec struct for clock_gettime, similar to Timespec
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimeSpec {
    /// seconds
    sec: usize,
    /// nano seconds
    nsec: usize,
}

/// TimeVal struct for gettimeofday
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimeVal {
    /// seconds
    sec: usize,
    /// microsecond
    usec: usize,
}

impl Syscall<'_> {
    /// finds the resolution (precision) of the specified clock clockid, and,
    /// if buffer is non-NULL, stores it in the struct timespec pointed to by buffer
    pub fn sys_clock_gettime(&self, clock: usize, mut buf: UserOutPtr<TimeSpec>) -> SysResult {
        info!("clock_gettime: id={:?} buf={:?}", clock, buf);
        // TODO: handle clock_settime
        let ts = TimeSpec::new();
        buf.write(ts)?;

        info!("TimeSpec: {:?}", ts);

        Ok(0)
    }

    /// get the time with second and microseconds
    pub fn sys_gettimeofday(
        &mut self,
        mut tv: UserOutPtr<TimeVal>,
        tz: UserInPtr<u8>,
    ) -> SysResult {
        info!("gettimeofday: tv: {:?}, tz: {:?}", tv, tz);
        // don't support tz
        if !tz.is_null() {
            return Err(LxError::EINVAL);
        }

        let timeval = TimeVal::new();
        tv.write(timeval)?;

        info!("TimeVal: {:?}", timeval);

        Ok(0)
    }

    /// get time in seconds
    #[cfg(target_arch = "x86_64")]
    pub fn sys_time(&mut self, mut time: UserOutPtr<u64>) -> SysResult {
        info!("time: time: {:?}", time);
        let sec = TimeSpec::new().sec;
        time.write(sec as u64)?;
        Ok(sec)
    }

    /// get resource usage
    /// currently only support ru_utime and ru_stime:
    /// - `ru_utime`: user CPU time used
    /// - `ru_stime`: system CPU time used
    pub fn sys_getrusage(&mut self, who: usize, mut rusage: UserOutPtr<RUsage>) -> SysResult {
        info!("getrusage: who: {}, rusage: {:?}", who, rusage);

        let new_rusage = RUsage {
            utime: TimeVal::new(),
            stime: TimeVal::new(),
        };
        rusage.write(new_rusage)?;
        Ok(0)
    }

    /// stores the current process times in the struct tms that buf points to
    pub fn sys_times(&mut self, mut buf: UserOutPtr<Tms>) -> SysResult {
        info!("times: buf: {:?}", buf);

        let tick = (TimeVal::new().sec * 1_000_000 + TimeVal::new().usec) / USEC_PER_TICK;

        let new_buf = Tms {
            tms_utime: 0,
            tms_stime: 0,
            tms_cutime: 0,
            tms_cstime: 0,
        };

        buf.write(new_buf)?;
        Ok(tick as usize)
    }
}

impl TimeVal {
    /// create TimeVal
    pub fn new() -> TimeVal {
        TimeSpec::new().into()
    }
}

impl TimeSpec {
    /// create TimeSpec
    pub fn new() -> TimeSpec {
        let time = timer_now();
        TimeSpec {
            sec: time.as_secs() as usize,
            nsec: (time.as_nanos() % 1_000_000_000) as usize,
        }
    }

    /// update TimeSpec for a file inode
    /// TODO: more precise; update when write
    pub fn update(inode: &Arc<dyn INode>) {
        let now = TimeSpec::new().into();
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
        Self::new()
    }
}

impl Default for TimeSpec {
    fn default() -> Self {
        Self::new()
    }
}

/// RUsage for sys_getrusage()
/// ignore other fields for now
#[repr(C)]
pub struct RUsage {
    /// user CPU time used
    utime: TimeVal,
    /// system CPU time used
    stime: TimeVal,
}

/// Tms for times()
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Tms {
    tms_utime: u64,  /* user time */
    tms_stime: u64,  /* system time */
    tms_cutime: u64, /* user time of children */
    tms_cstime: u64, /* system time of children */
}

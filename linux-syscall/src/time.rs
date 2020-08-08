//! Syscalls for time
//! - clock_gettime
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(missing_docs)]

const USEC_PER_TICK: usize = 10000;

use crate::Syscall;
use alloc::sync::Arc;
use core::time::Duration;
use kernel_hal::{timer_now, user::UserInPtr, user::UserOutPtr};
use linux_object::error::LxError;
use linux_object::error::SysResult;
use rcore_fs::vfs::*;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimeSpec {
    /// seconds
    sec: usize,
    /// nano seconds
    nsec: usize,
}

impl Syscall<'_> {
    /// finds the resolution (precision) of the specified clock clockid, and,
    /// if buffer is non-NULL, stores it in the struct timespec pointed to by buffer
    pub fn sys_clock_gettime(&self, clock: usize, mut buf: UserOutPtr<TimeSpec>) -> SysResult {
        info!("clock_gettime: id={:?} buf={:?}", clock, buf);
        // TODO: handle clock_settime
        let ts = TimeSpec::new();
        buf.write(ts)?;

        Ok(0)
    }

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
        Ok(0)
    }

    #[cfg(target_arch = "x86_64")]
    pub fn sys_time(&mut self, mut time: UserOutPtr<u64>) -> SysResult {
        let sec = TimeSpec::new().sec;
        time.write(sec as u64)?;
        Ok(sec)
    }

    pub fn sys_getrusage(&mut self, who: usize, mut rusage: UserOutPtr<RUsage>) -> SysResult {
        info!("getrusage: who: {}, rusage: {:?}", who, rusage);

        let new_rusage = RUsage {
            utime: TimeVal::new(),
            stime: TimeVal::new(),
        };
        rusage.write(new_rusage);
        Ok(0)
    }

    pub fn sys_times(&mut self, mut buf: UserOutPtr<Tms>) -> SysResult {
        info!("times: buf: {:?}", buf);

        let tick = 0; // unsafe { crate::trap::TICK as u64 };

        let new_buf = Tms {
            tms_utime: 0,
            tms_stime: 0,
            tms_cutime: 0,
            tms_cstime: 0,
        };
        // TODO: TICKS
        buf.write(new_buf);
        Ok(tick as usize)
    }
}

// 1ms msec
// 1us usec
// 1ns nsec
const USEC_PER_SEC: u64 = 1_000_000;
const MSEC_PER_SEC: u64 = 1_000;
const USEC_PER_MSEC: u64 = 1_000;
const NSEC_PER_USEC: u64 = 1_000;
const NSEC_PER_MSEC: u64 = 1_000_000;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimeVal {
    sec: usize,
    usec: usize,
}

impl TimeVal {
    pub fn new() -> TimeVal {
        TimeSpec::new().into()
    }

    pub fn to_msec(&self) -> u64 {
        (self.sec as u64) * MSEC_PER_SEC + (self.usec as u64) / USEC_PER_MSEC
    }
}

impl TimeSpec {
    pub fn new() -> TimeSpec {
        let time = timer_now();
        TimeSpec {
            sec: time.as_secs() as usize,
            nsec: (time.as_nanos() % 1_000_000_000) as usize,
        }
    }

    pub fn to_msec(&self) -> u64 {
        (self.sec as u64) * MSEC_PER_SEC + (self.nsec as u64) / NSEC_PER_MSEC
    }

    // TODO: more precise; update when write
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

    pub fn is_zero(&self) -> bool {
        self.sec == 0 && self.nsec == 0
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
            usec: self.nsec / NSEC_PER_USEC as usize,
        }
    }
}

// ignore other fields for now
#[repr(C)]
pub struct RUsage {
    utime: TimeVal,
    stime: TimeVal,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Tms {
    tms_utime: u64,  /* user time */
    tms_stime: u64,  /* system time */
    tms_cutime: u64, /* user time of children */
    tms_cstime: u64, /* system time of children */
}

#[cfg(test)]
mod test {

    #[test]
    fn test_time() {}
}

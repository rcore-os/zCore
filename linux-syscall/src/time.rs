//! Syscalls for time
//! - clock_gettime
//!
use crate::Syscall;
use kernel_hal::{user::UserInPtr, user::UserOutPtr};
use linux_object::error::LxError;
use linux_object::error::SysResult;
use linux_object::time::*;

const USEC_PER_TICK: usize = 10000;

impl Syscall<'_> {
    /// finds the resolution (precision) of the specified clock clockid, and
    /// if `buf` is non-NULL, stores it in the struct timespec pointed to by `buf`.
    ///
    /// the resolution of clocks depends on the implementation and cannot be configured by
    /// a particular process.
    ///
    /// currently `clock` only support `CLOCK_REALTIME`.
    /// 
    /// the `buf` argument is a wrapper of struct `timeval` which has fields:
    /// `sec: usize` and `usec: usize`
    /// 
    /// the SysResult is an alias for `LxError`
    /// which defined in `linux-object/src/error.rs`.
    /// 
    /// TODO: CLOCK_REALTIME_ALARM, CLOCK_REALTIME_COARSE, CLOCK_TAI, CLOCK_MONOTONIC, 
    /// CLOCK_MONOTONIC_COARSE, CLOCK_MONOTONIC_RAW, CLOCK_BOOTTIME, CLOCK_BOOTTIME_ALARM,
    /// CLOCK_PROCESS_CPUTIME_ID, CLOCK_THREAD_CPUTIME_ID.
    pub fn sys_clock_gettime(&self, clock: usize, mut buf: UserOutPtr<TimeSpec>) -> SysResult {
        info!("clock_gettime: id={:?} buf={:?}", clock, buf);
        // TODO: handle clock_settime
        let ts = TimeSpec::now();
        buf.write(ts)?;

        info!("TimeSpec: {:?}", ts);

        Ok(0)
    }

    /// get the time with second and microseconds.
    ///
    /// if `tz` is NULL return an error.
    /// 
    /// the `tv` argument is a wrapper of struct `timeval` which has fields:
    /// `sec: usize` and `usec: usize`
    /// 
    /// the `SysResult` is an alias for `LxError`
    /// which defined in `linux-object/src/error.rs`.
    
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

        let timeval = TimeVal::now();
        tv.write(timeval)?;

        info!("TimeVal: {:?}", timeval);

        Ok(0)
    }

    /// get time in seconds.
    ///
    /// returns the time as the number of seconds since the Epoch,
    /// 1970-01-01 00:00:00 +0000 (UTC).
    /// 
    /// the `time` argument is a wrapper of `u64`.
    /// 
    /// the `SysResult` is an alias for `LxError`
    /// which defined in `linux-object/src/error.rs`.
    #[cfg(target_arch = "x86_64")]
    pub fn sys_time(&mut self, mut time: UserOutPtr<u64>) -> SysResult {
        info!("time: time: {:?}", time);
        let sec = TimeSpec::now().sec;
        time.write(sec as u64)?;
        Ok(sec)
    }

    /// get resource usage
    /// currently only support ru_utime and ru_stime:
    /// - `ru_utime`: user CPU time used
    /// - `ru_stime`: system CPU time used
    ///
    /// the `rusage` argument is a wrapper of struct `RUsage` which has fields:
    /// `utime: TimeVal` and `stime: TimeVal`
    /// 
    /// the `SysResult` is an alias for `LxError`
    /// which defined in `linux-object/src/error.rs`.
    pub fn sys_getrusage(&mut self, who: usize, mut rusage: UserOutPtr<RUsage>) -> SysResult {
        info!("getrusage: who: {}, rusage: {:?}", who, rusage);

        let new_rusage = RUsage {
            utime: TimeVal::now(),
            stime: TimeVal::now(),
        };
        rusage.write(new_rusage)?;
        Ok(0)
    }

    /// stores the current process times in the struct tms that buf points to.
    ///
    /// the `buf` argument is a wrapper of `Tms`.
    /// 
    /// the `SysResult` is an alias for `LxError`
    /// which defined in `linux-object/src/error.rs`.
    pub fn sys_times(&mut self, mut buf: UserOutPtr<Tms>) -> SysResult {
        info!("times: buf: {:?}", buf);

        let tv = TimeVal::now();

        let tick = (tv.sec * 1_000_000 + tv.usec) / USEC_PER_TICK;

        let new_buf = Tms {
            tms_utime: 0,
            tms_stime: 0,
            tms_cutime: 0,
            tms_cstime: 0,
        };

        buf.write(new_buf)?;

        info!("tick: {:?}", tick);
        Ok(tick as usize)
    }
}

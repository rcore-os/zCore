//! Syscalls for time
//! - clock_gettime

use crate::Syscall;
use kernel_hal::{timer_now, user::UserOutPtr};
use linux_object::error::SysResult;

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

        let time = timer_now();
        let ts = TimeSpec {
            sec: time.as_secs() as usize,
            nsec: (time.as_nanos() % 1_000_000_000) as usize,
        };
        buf.write(ts)?;

        Ok(0)
    }
}

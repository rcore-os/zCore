//! Syscalls for time
//! - clock_gettime

use crate::Syscall;
use kernel_hal::{timer_now, user::UserOutPtr};
use linux_object::error::SysResult;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimeSpec {
    /// second
    sec: usize,
    /// nano second
    nsec: usize,
}

impl Syscall<'_> {
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

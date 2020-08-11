use crate::sys;
use crate::types::*;
use crate::error::*;

pub fn now() -> u64 {
    unsafe {
        sys::l4bridge_get_time_ts() as u64
    }
}

pub fn set_period(new_period: u64) -> KernelResult<()> {
    if unsafe {
        sys::l4bridge_timer_set_period_ts(new_period as _)
    } != 0 {
        Err(KernelError::BadTimerPeriod)
    } else {
        Ok(())
    }
}

pub fn wait() -> u64 {
    unsafe {
        sys::l4bridge_timer_wait_ts() as u64
    }
}
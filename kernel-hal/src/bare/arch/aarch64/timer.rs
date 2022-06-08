//! ARM Generic Timer.

use crate::timer::TICKS_PER_SEC;
use core::time::Duration;
use cortex_a::{asm::barrier, registers::*};
use tock_registers::interfaces::{Readable, Writeable};

pub fn timer_now() -> Duration {
    unsafe { barrier::isb(barrier::SY) }
    let cur_cnt = CNTPCT_EL0.get() * 1_000_000_000;
    let freq = CNTFRQ_EL0.get() as u64;
    Duration::from_nanos(cur_cnt / freq)
}

pub fn set_next_trigger() {
    CNTP_TVAL_EL0.set(CNTFRQ_EL0.get() / TICKS_PER_SEC);
}

pub fn init() {
    CNTP_CTL_EL0.write(CNTP_CTL_EL0::ENABLE::SET);
    set_next_trigger();
}

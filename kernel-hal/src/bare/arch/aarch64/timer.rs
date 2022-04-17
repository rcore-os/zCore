//! ARM Generic Timer.

use core::time::Duration;
use cortex_a::{
    registers::*,
    asm::barrier
};
use tock_registers::interfaces::{Readable, Writeable};
use super::gic::irq_set_mask;
use crate::timer::TICKS_PER_SEC;

const PHYS_TIMER_IRQ_NUM: usize = 30;

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
    irq_set_mask(PHYS_TIMER_IRQ_NUM, false);
}

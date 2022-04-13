use core::time::Duration;
use cortex_a::{
    registers::*,
    asm::barrier
};
use tock_registers::interfaces::Readable;

pub fn timer_now() -> Duration {
    unsafe { barrier::isb(barrier::SY) }
    let cur_cnt = CNTPCT_EL0.get() * 1_000_000_000;
    let freq = CNTFRQ_EL0.get() as u64;
    Duration::from_nanos(cur_cnt / freq)
}

use core::time::Duration;

fn get_cycle() -> u64 {
    riscv::register::time::read() as u64
}

pub(super) fn timer_set_next() {
    //let TIMEBASE: u64 = 100000;
    const TIMEBASE: u64 = 10_000_000;
    super::sbi::set_timer(get_cycle() + TIMEBASE);
}

pub(super) fn init() {
    timer_set_next();
}

pub(crate) fn timer_now() -> Duration {
    const FREQUENCY: u64 = 10_000_000; // ???
    let time = get_cycle();
    Duration::from_nanos(time * 1_000_000_000 / FREQUENCY as u64)
}

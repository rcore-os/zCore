use core::time::Duration;
use riscv::register::{sie, time};

fn get_cycle() -> u64 {
    time::read() as u64
    /*
    unsafe {
        MMIO_MTIME.read_volatile()
    }
    */
}

pub(super) fn timer_set_next() {
    //let TIMEBASE: u64 = 100000;
    const TIMEBASE: u64 = 10_000_000;
    super::sbi::set_timer(get_cycle() + TIMEBASE);
}

pub fn timer_now() -> Duration {
    const FREQUENCY: u64 = 10_000_000; // ???
    let time = get_cycle();
    //bare_println!("timer_now(): {:?}", time);
    Duration::from_nanos(time * 1_000_000_000 / FREQUENCY as u64)
}

pub(super) fn init() {
    unsafe { sie::set_stimer() };
    timer_set_next();
}

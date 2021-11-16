use core::time::Duration;

cfg_if! {
    if #[cfg(feature = "board-d1")] {
        const CLOCK_FREQ: u64 = 24_000_000; // C906
    } else {
        const CLOCK_FREQ: u64 = 10_000_000; // Qemu
    }
}
const TICKS_PER_SEC: u64 = 100;

fn get_cycle() -> u64 {
    riscv::register::time::read() as u64
}

pub(super) fn timer_set_next() {
    super::sbi::set_timer(get_cycle() + (CLOCK_FREQ / TICKS_PER_SEC));
}

pub(super) fn init() {
    timer_set_next();
}

pub(crate) fn timer_now() -> Duration {
    let time = get_cycle();
    Duration::from_nanos((time * (1_000_000_000 / CLOCK_FREQ)) as u64)
}

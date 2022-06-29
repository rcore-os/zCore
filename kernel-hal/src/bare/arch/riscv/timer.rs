use core::time::Duration;

fn get_cycle() -> u64 {
    riscv::register::time::read() as u64
}

pub(super) fn timer_set_next() {
    let cycles =
        super::cpu::cpu_frequency() as u64 * 1_000_000 / super::super::timer::TICKS_PER_SEC;
    sbi_rt::set_timer(get_cycle() + cycles);
}

pub(super) fn init() {
    timer_set_next();
}

pub(crate) fn timer_now() -> Duration {
    Duration::from_nanos(get_cycle() * 1000 / super::cpu::cpu_frequency() as u64)
}

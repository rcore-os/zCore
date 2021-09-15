use core::time::Duration;

pub fn timer_now() -> Duration {
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    Duration::from_nanos(tsc * 1000 / super::cpu::cpu_frequency() as u64)
}

use core::time::Duration;

pub fn timer_now() -> Duration {
    let cycle = unsafe { core::arch::x86_64::_rdtsc() };
    Duration::from_nanos(cycle * 1000 / super::cpu::cpu_frequency() as u64)
}

pub fn init() {
    let irq = crate::drivers::all_irq().first_unwrap();
    irq.apic_timer_enable();
}

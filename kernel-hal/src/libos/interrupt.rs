use crate::HalResult;

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {}
        fn intr_on() {}
        fn intr_off() {}
        fn intr_get() -> bool {
            false
        }
        fn send_ipi(cpuid: usize, reason: usize) -> HalResult {
            trace!("ipi [{}] => [{}]: {:x}", super::cpu::cpu_id(), cpuid, reason);
            Ok(())
        }
        fn ipi_reason() -> Vec<usize> {
            Vec::new()
        }
    }
}

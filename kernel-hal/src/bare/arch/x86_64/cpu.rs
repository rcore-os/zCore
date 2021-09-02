lazy_static::lazy_static! {
    static ref TSC_FREQUENCY: u16 = {
        const DEFAULT: u16 = 2600;
        if let Some(info) = raw_cpuid::CpuId::new().get_processor_frequency_info() {
            let f = info.processor_base_frequency();
            return if f == 0 { DEFAULT } else { f };
        }
        // FIXME: QEMU, AMD, VirtualBox
        DEFAULT
    };
}

hal_fn_impl! {
    impl mod crate::defs::cpu {
        fn cpu_id() -> u8 {
            super::apic::lapic_id()
        }

        fn cpu_frequency() -> u16 {
            *TSC_FREQUENCY
        }
    }
}

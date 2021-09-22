use raw_cpuid::CpuId;

lazy_static! {
    static ref TSC_FREQUENCY: u16 = {
        const DEFAULT: u16 = 4000;
        if let Some(info) = CpuId::new().get_processor_frequency_info() {
            let f = info.processor_base_frequency();
            return if f == 0 { DEFAULT } else { f };
        }
        // FIXME: QEMU, AMD, VirtualBox
        DEFAULT
    };
}

hal_fn_impl! {
    impl mod crate::hal_fn::cpu {
        fn cpu_id() -> u8 {
            CpuId::new()
                .get_feature_info()
                .unwrap()
                .initial_local_apic_id() as u8
        }

        fn cpu_frequency() -> u16 {
            *TSC_FREQUENCY
        }
    }
}

hal_fn_impl! {
    impl mod crate::defs::vdso {
        fn vdso_constants() -> VdsoConstants {
            let mut constants = vdso_constants_template();
            constants.physmem = super::mem_common::PMEM_SIZE as u64;
            constants
        }
    }
}

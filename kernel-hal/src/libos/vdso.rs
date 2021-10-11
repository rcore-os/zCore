hal_fn_impl! {
    impl mod crate::hal_fn::vdso {
        fn vdso_constants() -> VdsoConstants {
            let mut constants = vdso_constants_template();
            constants.physmem = super::mem::PMEM_SIZE as u64;
            constants
        }
    }
}

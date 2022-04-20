hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {}
        fn intr_on() {}
        fn intr_off() {}
        fn intr_get() -> bool {
            false
        }
    }
}

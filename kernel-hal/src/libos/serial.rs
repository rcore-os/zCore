hal_fn_impl! {
    impl mod crate::hal_fn::serial {
        fn serial_write_fmt(fmt: core::fmt::Arguments) {
            eprint!("{}", fmt);
        }
    }
}

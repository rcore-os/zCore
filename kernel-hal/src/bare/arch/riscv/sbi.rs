hal_fn_impl! {
    impl mod crate::hal_fn::console {
        fn console_write_early(s: &str) {
            for c in s.bytes() {
                #[allow(deprecated)]
                sbi_rt::legacy::console_putchar(c as _);
            }
        }
    }
}

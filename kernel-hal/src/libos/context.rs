pub use trapframe::syscall_fn_entry as syscall_entry;

hal_fn_impl! {
    impl mod crate::defs::context {
        fn context_run(context: &mut UserContext) {
            context.run_fncall();
        }
    }
}

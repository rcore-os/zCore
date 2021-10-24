cfg_if! {
    if #[cfg(not(target_os = "none"))] {
        #[path = "libos/entry.rs"]
        mod entry;
        #[path = "libos/consts.rs"]
        pub mod consts;
    } else if #[cfg(target_arch = "x86_64")] {
        #[path = "x86/entry.rs"]
        mod entry;
        #[path = "x86/consts.rs"]
        pub mod consts;
    } else if #[cfg(target_arch = "riscv64")] {
        #[path = "riscv/entry.rs"]
        mod entry;
        #[path = "riscv/consts.rs"]
        pub mod consts;
    }
}

#![allow(dead_code)]

cfg_if! {
    if #[cfg(not(feature = "libos"))] {
        pub(crate) mod irq_manager;
        pub(crate) mod page_table;
    }
}

pub(crate) mod init_once;

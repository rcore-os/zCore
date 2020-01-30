#![feature(lang_items)]
#![feature(naked_functions)]
#![feature(untagged_unions)]
#![feature(asm)]
#![feature(optin_builtin_traits)]
#![feature(panic_info_message)]
#![feature(global_asm)]
#![feature(alloc_prelude)]
#![deny(unused_must_use, unsafe_code, unused_imports)]
#![deny(stable_features)]
#![deny(ellipsis_inclusive_range_patterns)]
#![no_std]

extern crate alloc;
#[macro_use]
extern crate log;
extern crate rlibc;

#[macro_use]
pub mod logging;
pub mod lang;

use {
    buddy_system_allocator::{Heap, LockedHeapWithRescue},
    rboot::BootInfo,
};

#[no_mangle]
pub extern "C" fn _start(boot_info: &BootInfo) -> ! {
    logging::init();
    print!("Hello World!");
    info!("{:#x?}", boot_info);
    loop {}
}

/// Global heap allocator
///
/// Available after `memory::init()`.
///
/// It should be defined in memory mod, but in Rust `global_allocator` must be in root mod.
#[global_allocator]
static HEAP_ALLOCATOR: LockedHeapWithRescue = LockedHeapWithRescue::new(crate::enlarge_heap);

pub fn enlarge_heap(heap: &mut Heap) {}

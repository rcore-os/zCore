#![no_std]
#![feature(asm)]
#![feature(global_asm)]
#![deny(warnings, unused_must_use)]

extern crate alloc;
extern crate log;

use {
    alloc::{string::String, sync::Arc, vec::Vec},
    kernel_hal_unix::{switch_to_kernel, switch_to_user},
    linux_syscall::*,
    zircon_object::task::*,
};

pub fn run(libc_data: &[u8], args: Vec<String>, envs: Vec<String>) -> Arc<Process> {
    let job = Job::root();
    let proc = Process::create_linux(&job, "proc").unwrap();
    let thread = Thread::create(&proc, "thread", 0).unwrap();
    let loader = LinuxElfLoader {
        syscall_entry: syscall_entry as usize,
        stack_pages: 8,
    };
    let (entry, sp) = loader.load(&proc.vmar(), libc_data, args, envs).unwrap();

    thread
        .start(entry, sp, 0, 0)
        .expect("failed to start main thread");
    proc
}

extern "C" fn syscall_entry(
    num: u32,
    a0: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
) -> isize {
    unsafe {
        switch_to_kernel();
    }
    let syscall = Syscall {
        thread: Thread::current(),
        syscall_entry: syscall_entry as usize,
    };
    let ret = syscall.syscall(num, [a0, a1, a2, a3, a4, a5]);
    unsafe {
        switch_to_user();
    }
    ret
}

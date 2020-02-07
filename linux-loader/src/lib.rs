#![no_std]
#![feature(asm)]
#![feature(global_asm)]
#![deny(warnings, unused_must_use)]

extern crate alloc;
#[macro_use]
extern crate log;

use {
    alloc::{string::String, sync::Arc, vec::Vec},
    kernel_hal_unix::{syscall_entry, GeneralRegs},
    linux_syscall::*,
    zircon_object::task::*,
};

pub fn run(
    exec_path: &str,
    args: Vec<String>,
    envs: Vec<String>,
    rootfs: Arc<dyn FileSystem>,
) -> Arc<Process> {
    let job = Job::root();
    let proc = Process::create_linux(&job, rootfs.clone()).unwrap();
    let thread = Thread::create(&proc, "thread", 0).unwrap();
    let loader = LinuxElfLoader {
        syscall_entry: syscall_entry as usize,
        stack_pages: 8,
        root_inode: rootfs.root_inode(),
    };
    let inode = rootfs.root_inode().lookup(&exec_path).unwrap();
    let data = inode.read_as_vec().unwrap();
    let (entry, sp) = loader.load(&proc.vmar(), &data, args, envs).unwrap();

    thread
        .start(entry, sp, 0, 0)
        .expect("failed to start main thread");
    proc
}

#[no_mangle]
extern "C" fn handle_syscall(regs: &mut GeneralRegs) {
    trace!("syscall: {:#x?}", regs);
    let num = regs.rax as u32;
    let args = [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9];
    let mut syscall = Syscall {
        thread: Thread::current(),
        syscall_entry: syscall_entry as usize,
        regs,
    };
    regs.rax = syscall.syscall(num, args) as usize;
}

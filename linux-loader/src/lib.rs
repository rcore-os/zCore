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
        .start(entry, sp, 0, 0, 0)
        .expect("failed to start main thread");
    proc
}

extern "C" {
    fn syscall_entry();
}

#[cfg(target_os = "linux")]
global_asm!(
    r#"
.intel_syntax noprefix
syscall_entry:
    # save user stack to r10 (caller-saved)
    lea r10, [rsp + 8]

    # switch to kernel stack
    mov rsp, gs:64

    # pass argument in stack
    push r10                # user sp
    push [r10 - 8]          # user pc
    push rsp                # ptr to [pc, sp]
    push [r10]              # arg6
    call handle_syscall

    # back to user
    mov r10, [rsp + 16]     # load pc
    mov rsp, [rsp + 24]     # load sp
    jmp r10
"#
);

#[cfg(target_os = "macos")]
global_asm!(
    r#"
.intel_syntax noprefix
_syscall_entry:
    # save user stack to r10 (caller-saved)
    lea r10, [rsp + 8]

    # switch to kernel stack
    mov rsp, gs:48          # rsp = kernel gsbase
    mov rsp, [rsp - 48]     # rsp = kernel stack

    # pass argument in stack
    push r10                # user sp
    push [r10 - 8]          # user pc
    push rsp                # ptr to [pc, sp]
    push [r10]              # arg6
    call _handle_syscall

    # back to user
    mov r10, [rsp + 16]     # load pc
    mov rsp, [rsp + 24]     # load sp
    jmp r10
"#
);

#[no_mangle]
extern "C" fn handle_syscall(
    num: u32,
    a0: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    ptr: *mut usize,
) -> isize {
    unsafe {
        switch_to_kernel();
    }
    let syscall = Syscall {
        thread: Thread::current(),
        syscall_entry: syscall_entry as usize,
        ptr,
    };
    let ret = syscall.syscall(num, [a0, a1, a2, a3, a4, a5]);
    unsafe {
        switch_to_user();
    }
    ret
}

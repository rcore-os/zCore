#![feature(asm)]
#![feature(naked_functions)]

#[macro_use]
extern crate alloc;

use alloc::vec::Vec;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use xmas_elf::ElfFile;
use zircon_object::ipc::channel::*;
use zircon_object::object::*;
use zircon_object::task::*;
use zircon_object::vm::*;
use zircon_syscall::Syscall;
use zircon_userboot::VmarExt;

#[macro_use]
extern crate log;

fn main() {
    zircon_hal_unix::init();
    env_logger::init();

    let vmar = VmAddressRegion::new_root();
    const VBASE: usize = 0x400000000;
    const USERBOOT_SIZE: usize = 0x7000;
    const VDSO_SIZE: usize = 0x8000;

    // userboot
    let entry = {
        let vmar = vmar.create_child(VBASE, USERBOOT_SIZE).unwrap();
        let path = std::env::args()
            .nth(1)
            .expect("failed to read userboot path");
        let mut file = File::open(path).expect("failed to open file");
        let mut elf_data = Vec::new();
        file.read_to_end(&mut elf_data)
            .expect("failed to read file");
        let elf = ElfFile::new(&elf_data).unwrap();
        vmar.load_from_elf(&elf).unwrap();
        VBASE + elf.header.pt2.entry_point() as usize
    };

    // vdso
    {
        let vmar = vmar.create_child(VBASE + USERBOOT_SIZE, VDSO_SIZE).unwrap();
        let path = std::env::args().nth(2).expect("failed to read vdso path");
        let mut file = File::open(path).expect("failed to open file");
        let mut elf_data = Vec::new();
        file.read_to_end(&mut elf_data)
            .expect("failed to read file");
        let elf = ElfFile::new(&elf_data).unwrap();
        vmar.load_from_elf(&elf).unwrap();

        unsafe {
            // TODO: fix magic number
            // fill syscall entry
            ((VBASE + USERBOOT_SIZE + 0x4836) as *mut usize).write(syscall_entry as usize);
        }
    }

    // zbi
    let zbi_vmo = {
        let path = std::env::args().nth(3).expect("failed to read zbi path");
        let mut file = File::open(path).expect("failed to open file");
        let mut zbi_data = Vec::new();
        file.read_to_end(&mut zbi_data)
            .expect("failed to read file");
        let vmo = VMObjectPaged::new(zbi_data.len() / PAGE_SIZE + 1);
        vmo.write(0, &zbi_data);
        vmo
    };

    let job = Job::root();
    let proc = Process::create(&job, "proc", 0).unwrap();
    let thread = Thread::create(&proc, "thread", 0).unwrap();

    let (user_channel, kernel_channel) = Channel::create();
    let handle = Handle::new(user_channel, Rights::DEFAULT_CHANNEL);
    let cmdline = "\0";

    // FIXME: pass correct handles
    let mut handles = vec![Handle::new(proc.clone(), Rights::DUPLICATE); 13];
    handles[4] = Handle::new(zbi_vmo, Rights::DEFAULT_VMO);

    let msg = MessagePacket {
        data: Vec::from(cmdline),
        handles,
    };
    kernel_channel.write(msg);

    const STACK_SIZE: usize = 0x8000;
    let stack = Vec::<u8>::with_capacity(STACK_SIZE);
    let sp = stack.as_ptr() as usize + STACK_SIZE;
    proc.start(&thread, entry, sp, handle, 0)
        .expect("failed to start main thread");

    loop {
        std::thread::park();
    }
}

#[naked]
unsafe fn syscall_entry() {
    asm!("push rax" :::: "intel");
    #[cfg(not(target_os = "macos"))]
    asm!("call handle_syscall" :::: "intel");
    #[cfg(target_os = "macos")]
    asm!("call _handle_syscall" :::: "intel");
    asm!("add rsp, 8" :::: "intel");
}

#[no_mangle]
extern "C" fn handle_syscall(
    a0: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    num: u32, // pushed %eax
    _: usize, // return address
    a6: usize,
    a7: usize,
) -> isize {
    let syscall = Syscall {
        thread: Thread::current(),
    };
    syscall.syscall(num, [a0, a1, a2, a3, a4, a5, a6, a7])
}

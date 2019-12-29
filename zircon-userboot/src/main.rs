#![feature(asm)]
#![feature(naked_functions)]

use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use xmas_elf::ElfFile;
use zircon_object::task::*;
use zircon_object::vm::vmar::VmAddressRegion;
use zircon_syscall::Syscall;
use zircon_userboot::VmarExt;

#[macro_use]
extern crate log;

fn main() {
    zircon_hal_unix::init();
    env_logger::init();

    let vmar = VmAddressRegion::new_root();

    // userboot
    let entry_addr = {
        let vmar = vmar.create_child(0x300000000, 0x7000).unwrap();
        let path = std::env::args()
            .nth(1)
            .expect("failed to read userboot path");
        let mut file = File::open(path).expect("failed to open file");
        let mut elf_data = Vec::new();
        file.read_to_end(&mut elf_data)
            .expect("failed to read file");
        let elf = ElfFile::new(&elf_data).unwrap();
        vmar.load_from_elf(&elf).unwrap();
        elf.header.pt2.entry_point() as usize
    };

    // vdso
    {
        let vmar = vmar.create_child(0x300007000, 0x10000).unwrap();
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
            ((0x300000000usize + 0x7000 + 0x4836) as *mut usize).write(syscall_entry as usize);
        }
    }

    let job = Job::root();
    let proc = Process::create(&job, "proc", 0).unwrap();
    let thread = Thread::create(&proc, "thread", 0).unwrap();

    unsafe {
        THREAD = Some(thread);
    }

    let entry: extern "C" fn() = unsafe { core::mem::transmute(0x300000000 + entry_addr) };
    entry();
}

// TODO: support multi-thread
static mut THREAD: Option<Arc<Thread>> = None;

#[naked]
unsafe fn syscall_entry() {
    asm!("push rax" :::: "intel");
    #[cfg(not(target_os = "macos"))]
    asm!("call syscall" :::: "intel");
    #[cfg(target_os = "macos")]
    asm!("call _syscall" :::: "intel");
    asm!("add rsp, 8" :::: "intel");
}

#[no_mangle]
extern "C" fn syscall(
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
        thread: unsafe { THREAD.as_ref().unwrap().clone() },
    };
    syscall.syscall(num, [a0, a1, a2, a3, a4, a5, a6, a7])
}

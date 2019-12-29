#![feature(asm)]

use std::fs::File;
use std::io::Read;
use xmas_elf::ElfFile;
use zircon_object::vm::vmar::VmAddressRegion;
use zircon_object::ZxError;
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
    }

    unsafe {
        // TODO: fix magic number
        // fill syscall entry
        ((0x300000000usize + 0x7000 + 0x4836) as *mut usize).write(syscall as usize);
    }

    let entry: extern "C" fn() = unsafe { core::mem::transmute(0x300000000 + entry_addr) };
    entry();
}

extern "C" fn syscall(
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> isize {
    let num: u32;
    unsafe { asm!("" : "={eax}"(num) ::: "volatile") };
    info!(
        "syscall: num={}, args={:x?}",
        num,
        &[arg0, arg1, arg2, arg3, arg4, arg5]
    );
    const SYS_DEBUG_WRITE: u32 = 96;
    const SYS_PROCESS_EXIT: u32 = 38;
    match num {
        SYS_DEBUG_WRITE => {
            let buf = unsafe { std::slice::from_raw_parts(arg0 as _, arg1) };
            let s = std::str::from_utf8(buf).unwrap();
            println!("{}", s);
            0
        }
        SYS_PROCESS_EXIT => {
            panic!("zircon process exit");
        }
        _ => ZxError::NOT_SUPPORTED as isize,
    }
}

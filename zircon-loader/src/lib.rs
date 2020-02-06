#![no_std]
#![feature(asm)]
#![feature(global_asm)]
#![deny(warnings, unused_must_use)]

#[macro_use]
extern crate alloc;

extern crate log;

use {
    alloc::{sync::Arc, vec::Vec},
    kernel_hal_unix::{switch_to_kernel, switch_to_user},
    xmas_elf::{
        program::{Flags, ProgramHeader, SegmentData, Type},
        sections::SectionData,
        symbol_table::Entry,
        ElfFile,
    },
    zircon_object::{
        ipc::*,
        object::*,
        resource::{Resource, ResourceKind},
        task::*,
        vm::*,
        ZxError, ZxResult,
    },
    zircon_syscall::Syscall,
};

mod vdso;

pub fn run_userboot(
    userboot_data: &[u8],
    vdso_data: &[u8],
    zbi_data: &[u8],
    cmdline: &str,
) -> Arc<Process> {
    let job = Job::root();
    let proc = Process::create(&job, "proc", 0).unwrap();
    let thread = Thread::create(&proc, "thread", 0).unwrap();
    let resource = Resource::create("root", ResourceKind::ROOT);
    let vmar = proc.vmar();

    // userboot
    let (entry, vdso_addr) = {
        let elf = ElfFile::new(userboot_data).unwrap();
        let size = elf.load_segment_size();
        let vmar = vmar.create_child(None, size).unwrap();
        vmar.load_from_elf(&elf).unwrap();
        (
            vmar.addr() + elf.header.pt2.entry_point() as usize,
            vmar.addr() + size,
        )
    };

    // vdso
    let vdso_vmo = {
        let elf = ElfFile::new(vdso_data).unwrap();
        let size = elf.load_segment_size();
        let syscall_entry_offset = elf
            .get_symbol_address("zcore_syscall_entry")
            .expect("failed to locate syscall entry") as usize;
        let vmar = vmar.create_child_at(vdso_addr - vmar.addr(), size).unwrap();
        let first_vmo = vmar.load_from_elf(&elf).unwrap();
        // fill syscall entry
        extern "C" {
            fn syscall_entry();
        }
        first_vmo.write(
            syscall_entry_offset,
            &(syscall_entry as usize).to_ne_bytes(),
        );
        first_vmo
    };

    // zbi
    let zbi_vmo = {
        let vmo = VMObjectPaged::new(zbi_data.len() / PAGE_SIZE + 1);
        vmo.write(0, &zbi_data);
        vmo
    };

    let (user_channel, kernel_channel) = Channel::create();
    let handle = Handle::new(user_channel, Rights::DEFAULT_CHANNEL);

    // FIXME: pass correct handles
    let mut handles = vec![Handle::new(proc.clone(), Rights::DUPLICATE); 15];
    handles[2] = Handle::new(job, Rights::DEFAULT_JOB);
    handles[3] = Handle::new(resource, Rights::DEFAULT_RESOURCE);
    handles[4] = Handle::new(zbi_vmo, Rights::DEFAULT_VMO);
    handles[5] = Handle::new(vdso_vmo, Rights::DEFAULT_VMO);

    let mut data = Vec::from(cmdline);
    data.push(0);
    let msg = MessagePacket { data, handles };
    kernel_channel.write(msg).unwrap();

    const STACK_SIZE: usize = 0x8000;
    let stack = Vec::<u8>::with_capacity(STACK_SIZE);
    // WARN: align stack to 16B, then emulate a 'call' (push rip)
    let sp = ((stack.as_ptr() as usize + STACK_SIZE) & !0xf) - 8;
    proc.start(&thread, entry, sp, handle, 0)
        .expect("failed to start main thread");
    proc
}

#[cfg(not(target_os = "macos"))]
global_asm!(
    r#"
.intel_syntax noprefix
syscall_entry:
    push rax
    call handle_syscall
    add rsp, 8
    ret
"#
);

#[cfg(target_os = "macos")]
global_asm!(
    r#"
.intel_syntax noprefix
_syscall_entry:
    push rax
    call _handle_syscall
    add rsp, 8
    ret
"#
);

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
    unsafe {
        switch_to_kernel();
    }
    let syscall = Syscall {
        thread: Thread::current(),
    };
    let ret = syscall.syscall(num, [a0, a1, a2, a3, a4, a5, a6, a7]);
    unsafe {
        switch_to_user();
    }
    ret
}

pub trait ElfExt {
    fn load_segment_size(&self) -> usize;
    fn get_symbol_address(&self, symbol: &str) -> Option<u64>;
}

impl ElfExt for ElfFile<'_> {
    /// Get total size of all LOAD segments.
    fn load_segment_size(&self) -> usize {
        let pages = self
            .program_iter()
            .filter(|ph| ph.get_type().unwrap() == Type::Load)
            .map(|ph| pages(ph.mem_size() as usize))
            .sum::<usize>();
        pages * PAGE_SIZE
    }

    /// Get address of the given `symbol`.
    fn get_symbol_address(&self, symbol: &str) -> Option<u64> {
        for section in self.section_iter() {
            if let SectionData::SymbolTable64(entries) = section.get_data(self).unwrap() {
                for e in entries {
                    if e.get_name(self).unwrap() == symbol {
                        return Some(e.value());
                    }
                }
            }
        }
        None
    }
}

pub trait VmarExt {
    fn load_from_elf(&self, elf: &ElfFile) -> ZxResult<Arc<VMObjectPaged>>;
}

impl VmarExt for VmAddressRegion {
    /// Create `VMObject` from all LOAD segments of `elf` and map them to this VMAR.
    /// Return the first `VMObject`.
    fn load_from_elf(&self, elf: &ElfFile) -> ZxResult<Arc<VMObjectPaged>> {
        let mut first_vmo = None;
        for ph in elf.program_iter() {
            if ph.get_type().unwrap() != Type::Load {
                continue;
            }
            let vmo = make_vmo(&elf, ph)?;
            let len = vmo.len();
            let flags = ph.flags().to_mmu_flags();
            self.map_at(ph.virtual_addr() as usize, vmo.clone(), 0, len, flags)?;
            first_vmo.get_or_insert(vmo);
        }
        Ok(first_vmo.unwrap())
    }
}

trait FlagsExt {
    fn to_mmu_flags(&self) -> MMUFlags;
}

impl FlagsExt for Flags {
    fn to_mmu_flags(&self) -> MMUFlags {
        let mut flags = MMUFlags::empty();
        if self.is_read() {
            flags.insert(MMUFlags::READ);
        }
        if self.is_write() {
            flags.insert(MMUFlags::WRITE);
        }
        if self.is_execute() {
            flags.insert(MMUFlags::EXECUTE);
        }
        flags
    }
}

fn make_vmo(elf: &ElfFile, ph: ProgramHeader) -> ZxResult<Arc<VMObjectPaged>> {
    assert_eq!(ph.get_type().unwrap(), Type::Load);
    let pages = pages(ph.mem_size() as usize);
    let vmo = VMObjectPaged::new(pages);
    let data = match ph.get_data(&elf).unwrap() {
        SegmentData::Undefined(data) => data,
        _ => return Err(ZxError::INVALID_ARGS),
    };
    vmo.write(0, data);
    Ok(vmo)
}

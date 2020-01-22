#![no_std]
#![feature(asm)]
#![feature(global_asm)]
#![deny(warnings, unused_must_use)]

extern crate alloc;
extern crate log;

use {
    alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec},
    linux_syscall::Syscall,
    xmas_elf::{
        program::{Flags, ProgramHeader, SegmentData, Type},
        sections::SectionData,
        symbol_table::{DynEntry64, Entry},
        ElfFile,
    },
    zircon_hal_unix::swap_fs,
    zircon_object::{task::*, vm::*, ZxError, ZxResult},
};

mod abi;

pub fn run(
    _ldso_data: &[u8],
    libc_data: &[u8],
    mut args: Vec<String>,
    envs: Vec<String>,
) -> Arc<Process> {
    let job = Job::root();
    let proc = Process::create(&job, "proc", 0).unwrap();
    let thread = Thread::create(&proc, "thread", 0).unwrap();
    let vmar = proc.vmar();

    const VBASE: usize = 0x4_00000000;

    // libc.so
    let entry = {
        let elf = ElfFile::new(libc_data).unwrap();
        let size = elf.load_segment_size();
        let syscall_entry_offset = elf
            .get_symbol_address("rcore_syscall_entry")
            .expect("failed to locate syscall entry") as usize;
        let vmar = vmar.create_child(VBASE, size).unwrap();
        let vmo = vmar.load_from_elf(&elf).unwrap();
        // fill syscall entry
        extern "C" {
            fn syscall_entry();
        }
        vmo.write(
            syscall_entry_offset,
            &(syscall_entry as usize).to_ne_bytes(),
        );
        elf.relocate(VBASE).unwrap();
        VBASE + elf.header.pt2.entry_point() as usize
    };

    // ld.so
    //    let entry = {
    //        let elf = ElfFile::new(ldso_data).unwrap();
    //        let size = elf.load_segment_size();
    //        let vmar = vmar.create_child(VBASE + 0x400000, size).unwrap();
    //        let first_vmo = vmar.load_from_elf(&elf).unwrap();
    //        elf.relocate(VBASE + 0x400000).unwrap();
    //        VBASE + 0x400000 + elf.header.pt2.entry_point() as usize
    //    };

    const STACK_SIZE: usize = 0x8000;
    let stack = Vec::<u8>::with_capacity(STACK_SIZE);
    let mut sp = (stack.as_ptr() as usize + STACK_SIZE) & !0xf;

    args.insert(0, String::from("libc.so"));
    let info = abi::ProcInitInfo {
        args,
        envs,
        auxv: {
            let mut map = BTreeMap::new();
            map.insert(abi::AT_BASE, VBASE);
            map.insert(abi::AT_PAGESZ, PAGE_SIZE);
            map
        },
    };
    sp = unsafe { info.push_at(sp) };

    thread
        .start(entry, sp, 0, 0)
        .expect("failed to start main thread");
    proc
}

#[cfg(not(target_os = "macos"))]
global_asm!(
    r#"
.intel_syntax noprefix
syscall_entry:
    push rbp
    push rax
    call handle_syscall
    add rsp, 16
    ret
"#
);

#[cfg(target_os = "macos")]
global_asm!(
    r#"
.intel_syntax noprefix
_syscall_entry:
    push rbp
    push rax
    call _handle_syscall
    add rsp, 16
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
) -> isize {
    swap_fs();
    let syscall = Syscall {
        thread: Thread::current(),
    };
    let ret = syscall.syscall(num, [a0, a1, a2, a3, a4, a5]);
    swap_fs();
    ret
}

pub trait ElfExt {
    fn load_segment_size(&self) -> usize;
    fn get_symbol_address(&self, symbol: &str) -> Option<u64>;
    fn dynsym(&self) -> Result<&[DynEntry64], &'static str>;
    fn relocate(&self, base: usize) -> Result<(), &'static str>;
}

impl ElfExt for ElfFile<'_> {
    /// Get total size of all LOAD segments.
    fn load_segment_size(&self) -> usize {
        self.program_iter()
            .filter(|ph| ph.get_type().unwrap() == Type::Load)
            .map(|ph| pages((ph.virtual_addr() + ph.mem_size()) as usize))
            .max()
            .unwrap()
            * PAGE_SIZE
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

    fn dynsym(&self) -> Result<&[DynEntry64], &'static str> {
        match self
            .find_section_by_name(".dynsym")
            .ok_or(".dynsym not found")?
            .get_data(self)
            .map_err(|_| "corrupted .dynsym")?
        {
            SectionData::DynSymbolTable64(dsym) => Ok(dsym),
            _ => Err("bad .dynsym"),
        }
    }

    fn relocate(&self, base: usize) -> Result<(), &'static str> {
        let data = self
            .find_section_by_name(".rela.dyn")
            .ok_or(".rela.dyn not found")?
            .get_data(self)
            .map_err(|_| "corrupted .rela.dyn")?;
        let entries = match data {
            SectionData::Rela64(entries) => entries,
            _ => return Err("bad .rela.dyn"),
        };
        let dynsym = self.dynsym()?;
        for entry in entries {
            const REL_GOT: u32 = 6;
            const REL_PLT: u32 = 7;
            const REL_RELATIVE: u32 = 8;
            match entry.get_type() {
                REL_GOT | REL_PLT => {
                    let dynsym = &dynsym[entry.get_symbol_table_index() as usize];
                    let symval = if dynsym.shndx() == 0 {
                        let name = dynsym.get_name(self)?;
                        panic!("need to find symbol: {:?}", name);
                    } else {
                        base + dynsym.value() as usize
                    };
                    let value = symval + entry.get_addend() as usize;
                    unsafe {
                        let ptr = (base + entry.get_offset() as usize) as *mut usize;
                        ptr.write(value);
                    }
                }
                REL_RELATIVE => {
                    let value = base + entry.get_addend() as usize;
                    unsafe {
                        let ptr = (base + entry.get_offset() as usize) as *mut usize;
                        ptr.write(value);
                    }
                }
                t => unimplemented!("unknown type: {}", t),
            }
        }
        Ok(())
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
            let offset = ph.virtual_addr() as usize / PAGE_SIZE * PAGE_SIZE;
            let flags = ph.flags().to_mmu_flags();
            self.map(offset, vmo.clone(), 0, vmo.len(), flags)?;
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
    let page_offset = ph.virtual_addr() as usize % PAGE_SIZE;
    let pages = pages(ph.mem_size() as usize + page_offset);
    let vmo = VMObjectPaged::new(pages);
    let data = match ph.get_data(&elf).unwrap() {
        SegmentData::Undefined(data) => data,
        _ => return Err(ZxError::INVALID_ARGS),
    };
    vmo.write(page_offset, data);
    Ok(vmo)
}

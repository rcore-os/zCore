//! ELF loading of Zircon and Linux.
use crate::{error::*, vm::*};
use alloc::sync::Arc;
use xmas_elf::{
    program::{Flags, ProgramHeader, SegmentData, Type},
    sections::SectionData,
    symbol_table::{DynEntry64, Entry},
    ElfFile,
};

/// Extensional ELF loading methods for `VmAddressRegion`.
pub trait VmarExt {
    /// Create `VMObject` from all LOAD segments of `elf` and map them to this VMAR.
    /// Return the first `VMObject`.
    fn load_from_elf(&self, elf: &ElfFile) -> ZxResult<Arc<VmObject>>;
    /// Same as `load_from_elf`, but the `vmo` is an existing one instead of a lot of new ones.
    fn map_from_elf(&self, elf: &ElfFile, vmo: Arc<VmObject>) -> ZxResult;
}

impl VmarExt for VmAddressRegion {
    fn load_from_elf(&self, elf: &ElfFile) -> ZxResult<Arc<VmObject>> {
        let mut first_vmo = None;
        for ph in elf.program_iter() {
            if ph.get_type().unwrap() != Type::Load {
                continue;
            }
            let vmo = make_vmo(elf, ph)?;
            let offset = ph.virtual_addr() as usize / PAGE_SIZE * PAGE_SIZE;
            let flags = ph.flags().to_mmu_flags();
            trace!("ph:{:#x?}, offset:{:#x?}, flags:{:#x?}", ph, offset, flags);
            //映射vmo物理内存块到 VMAR
            self.map_at(offset, vmo.clone(), 0, vmo.len(), flags)?;
            debug!("Map [{:x}, {:x})", offset, offset + vmo.len());
            first_vmo.get_or_insert(vmo);
        }
        Ok(first_vmo.unwrap())
    }
    fn map_from_elf(&self, elf: &ElfFile, vmo: Arc<VmObject>) -> ZxResult {
        for ph in elf.program_iter() {
            if ph.get_type().unwrap() != Type::Load {
                continue;
            }
            let offset = ph.virtual_addr() as usize;
            let flags = ph.flags().to_mmu_flags();
            let vmo_offset = pages(ph.physical_addr() as usize) * PAGE_SIZE;
            let len = pages(ph.mem_size() as usize) * PAGE_SIZE;
            self.map_at(offset, vmo.clone(), vmo_offset, len, flags)?;
        }
        Ok(())
    }
}

trait FlagsExt {
    fn to_mmu_flags(&self) -> MMUFlags;
}

impl FlagsExt for Flags {
    fn to_mmu_flags(&self) -> MMUFlags {
        let mut flags = MMUFlags::USER;
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

fn make_vmo(elf: &ElfFile, ph: ProgramHeader) -> ZxResult<Arc<VmObject>> {
    assert_eq!(ph.get_type().unwrap(), Type::Load);
    let page_offset = ph.virtual_addr() as usize % PAGE_SIZE;
    // (VirtAddr余数 + MemSiz)的pages
    let pages = pages(ph.mem_size() as usize + page_offset);
    trace!(
        "VmObject new pages: {:#x}, virtual_addr: {:#x}",
        pages,
        page_offset
    );
    let vmo = VmObject::new_paged(pages);
    let data = match ph.get_data(elf).unwrap() {
        SegmentData::Undefined(data) => data,
        _ => return Err(ZxError::INVALID_ARGS),
    };
    //调用 VMObjectTrait.write, 分配物理内存，后写入程序数据
    vmo.write(page_offset, data)?;
    Ok(vmo)
}

/// Extensional ELF loading methods for `ElfFile`.
pub trait ElfExt {
    /// Get total size of all LOAD segments.
    fn load_segment_size(&self) -> usize;
    /// Get address of the given `symbol`.
    fn get_symbol_address(&self, symbol: &str) -> Option<u64>;
    /// Get the program interpreter path name.
    fn get_interpreter(&self) -> Result<&str, &str>;
    /// Get address of elf phdr
    fn get_phdr_vaddr(&self) -> Option<u64>;
    /// Get the symbol table for dynamic linking (.dynsym section).
    fn dynsym(&self) -> Result<&[DynEntry64], &'static str>;
    /// Relocate according to the dynamic relocation section (.rel.dyn section).
    fn relocate(&self, vmar: Arc<VmAddressRegion>) -> Result<(), &'static str>;
}

impl ElfExt for ElfFile<'_> {
    fn load_segment_size(&self) -> usize {
        self.program_iter()
            .filter(|ph| ph.get_type().unwrap() == Type::Load)
            .map(|ph| pages((ph.virtual_addr() + ph.mem_size()) as usize))
            .max()
            .unwrap_or(0)
            * PAGE_SIZE
    }

    fn get_symbol_address(&self, symbol: &str) -> Option<u64> {
        for section in self.section_iter() {
            if let Ok(SectionData::SymbolTable64(entries)) = section.get_data(self) {
                for e in entries {
                    if let Ok(name) = e.get_name(self) {
                        if name == symbol {
                            return Some(e.value());
                        }
                    }
                }
            }
        }
        None
    }

    fn get_interpreter(&self) -> Result<&str, &str> {
        let header = self
            .program_iter()
            .find(|ph| ph.get_type() == Ok(Type::Interp))
            .ok_or("no interp header")?;
        let data = match header.get_data(self)? {
            SegmentData::Undefined(data) => data,
            _ => return Err("bad interp"),
        };
        let len = (0..).find(|&i| data[i] == 0).unwrap();
        let path = core::str::from_utf8(&data[..len]).map_err(|_| "failed to convert to utf8")?;
        Ok(path)
    }

    /*
     * [ ERROR ] page fualt from user mode 0x40 READ
     */

    fn get_phdr_vaddr(&self) -> Option<u64> {
        if let Some(phdr) = self
            .program_iter()
            .find(|ph| ph.get_type() == Ok(Type::Phdr))
        {
            // if phdr exists in program header, use it
            Some(phdr.virtual_addr())
        } else if let Some(elf_addr) = self
            .program_iter()
            .find(|ph| ph.get_type() == Ok(Type::Load) && ph.offset() == 0)
        {
            // otherwise, check if elf is loaded from the beginning, then phdr can be inferred.
            Some(elf_addr.virtual_addr() + self.header.pt2.ph_offset())
        } else {
            warn!("elf: no phdr found, tls might not work");
            None
        }
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

    #[allow(unsafe_code)]
    fn relocate(&self, vmar: Arc<VmAddressRegion>) -> Result<(), &'static str> {
        // Symbol-resolving relocations (write `S + A`).
        // x86_64
        const REL_GOT: u32 = 6; // R_X86_64_GLOB_DAT
        const REL_PLT: u32 = 7; // R_X86_64_JUMP_SLOT
        const R_X86_64_64: u32 = 1;
        // riscv64
        const R_RISCV_64: u32 = 2;
        // aarch64
        const R_AARCH64_GLOBAL_DATA: u32 = 0x401;
        const R_AARCH64_JUMP_SLOT: u32 = 0x402;

        // Base-relative relocations (write `B + A`).
        // x86_64
        const REL_RELATIVE: u32 = 8; // R_X86_64_RELATIVE
                                     // riscv64
        const R_RISCV_RELATIVE: u32 = 3;
        // aarch64
        const R_AARCH64_RELATIVE: u32 = 0x403;

        let base = vmar.addr();
        // `.dynsym` may be absent for binaries that only carry RELATIVE
        // relocations; resolve it lazily so those still get applied.
        let dynsym = self.dynsym().ok();
        let mut found_any = false;

        // One-entry mapping cache. Relocation targets are heavily clustered
        // (GOT / PLT / .data.rel.ro live in one or two mappings), while
        // `Vmar::write_memory` re-resolves the mapping with a linear VMAR
        // scan on EVERY call — O(entries × mappings) per exec for a PIE
        // binary with thousands of R_*_RELATIVE entries.
        let mut cached: Option<Arc<VmMapping>> = None;
        let write_word = |cached: &mut Option<Arc<VmMapping>>,
                          addr: usize,
                          value: usize|
         -> Result<(), &'static str> {
            let bytes = value.to_ne_bytes();
            if let Some(map) = cached.as_ref() {
                if map
                    .write_memory_if_contains(addr, &bytes)
                    .map_err(|_| "Invalid Vmar")?
                {
                    return Ok(());
                }
            }
            let map = vmar.find_mapping(addr).ok_or("Invalid Vmar")?;
            if !map
                .write_memory_if_contains(addr, &bytes)
                .map_err(|_| "Invalid Vmar")?
            {
                // Boundary case (write straddles the mapping end): preserve the
                // old clamped-partial-write behaviour.
                vmar.write_memory(addr, &bytes)
                    .map_err(|_| "Invalid Vmar")?;
            }
            *cached = Some(map);
            Ok(())
        };

        // Apply both the general dynamic relocations (`.rela.dyn`) and the PLT
        // relocations (`.rela.plt`). The latter holds the JUMP_SLOT entries that
        // back the procedure linkage table; skipping it leaves call targets
        // pointing at unrelocated stubs (observed as a jump to a low address and
        // an Invalid Opcode #UD fault).
        for &sec_name in [".rela.dyn", ".rela.plt"].iter() {
            let section = match self.find_section_by_name(sec_name) {
                Some(section) => section,
                None => continue,
            };
            let entries = match section
                .get_data(self)
                .map_err(|_| "corrupted relocation section")?
            {
                SectionData::Rela64(entries) => entries,
                _ => continue,
            };
            found_any = true;
            for entry in entries.iter() {
                match entry.get_type() {
                    REL_GOT
                    | REL_PLT
                    | R_X86_64_64
                    | R_RISCV_64
                    | R_AARCH64_GLOBAL_DATA
                    | R_AARCH64_JUMP_SLOT => {
                        let dynsym = match dynsym {
                            Some(dynsym) => dynsym,
                            None => {
                                warn!("relocate: symbol relocation but no .dynsym; skipping");
                                continue;
                            }
                        };
                        let sym = &dynsym[entry.get_symbol_table_index() as usize];
                        // An undefined symbol (shndx == 0) is resolved later by the
                        // dynamic linker in user space (or is simply unavailable to
                        // the in-kernel loader). Skip it instead of panicking — a
                        // user binary must never be able to crash the kernel.
                        if sym.shndx() == 0 {
                            let name = sym.get_name(self).unwrap_or("<unknown>");
                            warn!("relocate: undefined symbol {:?}, skipping", name);
                            continue;
                        }
                        let symval = base + sym.value() as usize;
                        let value = symval + entry.get_addend() as usize;
                        let addr = base + entry.get_offset() as usize;
                        trace!("GOT write: {:#x} @ {:#x}", value, addr);
                        write_word(&mut cached, addr, value)?;
                    }
                    REL_RELATIVE | R_RISCV_RELATIVE | R_AARCH64_RELATIVE => {
                        let value = base + entry.get_addend() as usize;
                        let addr = base + entry.get_offset() as usize;
                        trace!("RELATIVE write: {:#x} @ {:#x}", value, addr);
                        write_word(&mut cached, addr, value)?;
                    }
                    // Unsupported relocation type (e.g. TLS or IFUNC relocations).
                    // Log and skip rather than `unimplemented!()`, which would
                    // panic the whole kernel because of one user program.
                    other => {
                        warn!(
                            "relocate: skipping unsupported relocation type {} in {}",
                            other, sec_name
                        );
                    }
                }
            }
        }

        if found_any {
            Ok(())
        } else {
            Err(".rela.dyn not found")
        }
    }
}

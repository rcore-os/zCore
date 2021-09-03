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
        let data = self
            .find_section_by_name(".rela.dyn")
            .ok_or(".rela.dyn not found")?
            .get_data(self)
            .map_err(|_| "corrupted .rela.dyn")?;
        let entries = match data {
            SectionData::Rela64(entries) => entries,
            _ => return Err("bad .rela.dyn"),
        };
        let base = vmar.addr();
        let dynsym = self.dynsym()?;
        for entry in entries.iter() {
            const REL_GOT: u32 = 6;
            const REL_PLT: u32 = 7;
            const REL_RELATIVE: u32 = 8;
            const R_RISCV_64: u32 = 2;
            const R_RISCV_RELATIVE: u32 = 3;
            match entry.get_type() {
                REL_GOT | REL_PLT | R_RISCV_64 => {
                    let dynsym = &dynsym[entry.get_symbol_table_index() as usize];
                    let symval = if dynsym.shndx() == 0 {
                        let name = dynsym.get_name(self)?;
                        panic!("need to find symbol: {:?}", name);
                    } else {
                        base + dynsym.value() as usize
                    };
                    let value = symval + entry.get_addend() as usize;
                    let addr = base + entry.get_offset() as usize;
                    debug!("GOT write: {:#x} @ {:#x}", value, addr);
                    vmar.write_memory(addr, &value.to_ne_bytes())
                        .map_err(|_| "Invalid Vmar")?;
                }
                REL_RELATIVE | R_RISCV_RELATIVE => {
                    let value = base + entry.get_addend() as usize;
                    let addr = base + entry.get_offset() as usize;
                    debug!("RELATIVE write: {:#x} @ {:#x}", value, addr);
                    vmar.write_memory(addr, &value.to_ne_bytes())
                        .map_err(|_| "Invalid Vmar")?;
                }
                t => unimplemented!("unknown type: {}", t),
            }
        }
        // panic!("STOP");
        Ok(())
    }
}

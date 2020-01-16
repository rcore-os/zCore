#![no_std]
#![deny(warnings, unused_must_use)]

extern crate alloc;

extern crate log;

use {
    alloc::sync::Arc,
    xmas_elf::{
        program::{ProgramHeader, SegmentData, Type},
        sections::SectionData,
        symbol_table::Entry,
        ElfFile,
    },
    zircon_object::{vm::*, ZxError, ZxResult},
};

mod vdso;

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
            self.map(ph.virtual_addr() as usize, vmo.clone(), 0, len)?;
            first_vmo.get_or_insert(vmo);
        }
        Ok(first_vmo.unwrap())
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

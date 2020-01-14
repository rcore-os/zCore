#![no_std]
#![deny(warnings, unused_must_use)]

extern crate alloc;

extern crate log;

use {
    alloc::sync::Arc,
    xmas_elf::program::{ProgramHeader, SegmentData, Type},
    xmas_elf::ElfFile,
    zircon_object::vm::*,
    zircon_object::{ZxError, ZxResult},
};

mod vdso;

pub trait VmarExt {
    fn load_from_elf(&self, elf: &ElfFile) -> ZxResult<Arc<VMObjectPaged>>;
}

impl VmarExt for VmAddressRegion {
    fn load_from_elf(&self, elf: &ElfFile) -> Result<Arc<VMObjectPaged>, ZxError> {
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
    let pages = (ph.mem_size() as usize + PAGE_SIZE - 1) / PAGE_SIZE;
    let vmo = VMObjectPaged::new(pages);
    let data = match ph.get_data(&elf).unwrap() {
        SegmentData::Undefined(data) => data,
        _ => return Err(ZxError::INVALID_ARGS),
    };
    vmo.write(0, data);
    Ok(vmo)
}

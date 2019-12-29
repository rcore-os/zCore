#![no_std]
#![deny(unused_must_use)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use alloc::sync::Arc;
use xmas_elf::program::{ProgramHeader, SegmentData, Type};
use xmas_elf::ElfFile;
use zircon_object::vm::vmar::VmAddressRegion;
use zircon_object::vm::vmo::{VMObject, VMObjectPaged};
use zircon_object::vm::PAGE_SIZE;
use zircon_object::{ZxError, ZxResult};

mod vdso;

pub trait VmarExt {
    fn load_from_elf(&self, elf: &ElfFile) -> ZxResult<()>;
}

impl VmarExt for VmAddressRegion {
    fn load_from_elf(&self, elf: &ElfFile) -> Result<(), ZxError> {
        for ph in elf.program_iter() {
            if ph.get_type().unwrap() != Type::Load {
                continue;
            }
            let vmo = make_vmo(&elf, ph)?;
            let len = vmo.len();
            self.map(ph.virtual_addr() as usize, vmo, 0, len)?;
        }
        Ok(())
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

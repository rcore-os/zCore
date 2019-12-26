use super::*;
use crate::object::*;

/// Virtual Memory Objects
pub trait VMObject: KernelObject {
    fn read(&self, offset: usize, buf: &mut [u8]);
    fn write(&self, offset: usize, buf: &[u8]);
    fn size(&self) -> usize;
    fn set_size(&self);
}

/// The main VM object type, holding a list of pages.
pub struct VMObjectPaged {
    base: KObjectBase,
    pages: usize,
    resizable: bool,
}

impl_kobject!(VMObjectPaged);

impl VMObject for VMObjectPaged {
    fn read(&self, offset: usize, buf: &mut [u8]) {
        unimplemented!()
    }

    fn write(&self, offset: usize, buf: &[u8]) {
        unimplemented!()
    }

    fn size(&self) -> usize {
        unimplemented!()
    }

    fn set_size(&self) {
        unimplemented!()
    }
}

/// VMO representing a physical range of memory.
pub struct VMObjectPhysical {
    base: KObjectBase,
    paddr: PhysAddr,
    pages: usize,
}

impl_kobject!(VMObjectPhysical);

impl VMObject for VMObjectPhysical {
    fn read(&self, offset: usize, buf: &mut [u8]) {
        unimplemented!()
    }

    fn write(&self, offset: usize, buf: &[u8]) {
        unimplemented!()
    }

    fn size(&self) -> usize {
        unimplemented!()
    }

    fn set_size(&self) {
        unimplemented!()
    }
}

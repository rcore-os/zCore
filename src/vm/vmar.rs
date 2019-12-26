use super::*;
use crate::object::*;
use crate::vm::vmo::VMObject;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// Virtual Memory Address Regions
pub struct VmAddressRegion {
    base: KObjectBase,
    addr: VirtAddr,
    size: usize,
    inner: Mutex<VmarInner>,
}

impl_kobject!(VmAddressRegion);

/// The mutable part of `VmAddressRegion`.
#[derive(Default)]
struct VmarInner {
    children: Vec<Arc<VmAddressRegion>>,
    mappings: Vec<Arc<VmMapping>>,
}

/// Virtual Memory Mapping
pub struct VmMapping {
    addr: VirtAddr,
    size: usize,
    vmo: Arc<dyn VMObject>,
    vmo_offset: usize,
}

impl VmAddressRegion {
    /// Create a new root VMAR.
    pub fn new_root() -> Arc<Self> {
        Arc::new(VmAddressRegion {
            base: KObjectBase::new(),
            addr: 0,
            size: 0x8000_00000000,
            inner: Mutex::new(VmarInner::default()),
        })
    }

    /// Create a child VMAR at `offset` with `len`.
    ///
    /// The `offset` and `len` should be page aligned,
    /// or an `INVALID_ARGS` error will be returned.
    pub fn create_child(&self, offset: usize, len: usize) -> ZxResult<Arc<Self>> {
        if !page_aligned(offset) || !page_aligned(len) {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut inner = self.inner.lock();
        if !self.test_map(&inner, offset, len) {
            return Err(ZxError::INVALID_ARGS);
        }
        let child = Arc::new(VmAddressRegion {
            base: KObjectBase::new(),
            addr: self.addr + offset,
            size: len,
            inner: Mutex::new(VmarInner::default()),
        });
        inner.children.push(child.clone());
        Ok(child)
    }

    /// Map the `vmo` into this VMAR.
    pub fn map(
        &self,
        offset: usize,
        vmo: Arc<dyn VMObject>,
        vmo_offset: usize,
        len: usize,
    ) -> ZxResult<Arc<VmMapping>> {
        if !page_aligned(offset) || !page_aligned(vmo_offset) || !page_aligned(len) {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut inner = self.inner.lock();
        if !self.test_map(&inner, offset, len) {
            return Err(ZxError::INVALID_ARGS);
        }
        let mapping = Arc::new(VmMapping {
            addr: self.addr + offset,
            size: len,
            vmo,
            vmo_offset,
        });
        inner.mappings.push(mapping.clone());
        Ok(mapping)
    }

    pub fn unmap(&self) {
        unimplemented!()
    }

    pub fn protect(&self) {
        unimplemented!()
    }

    pub fn destroy(&self) {
        unimplemented!()
    }

    /// Test if can create a new mapping at `offset` with `len`.
    fn test_map(&self, inner: &VmarInner, offset: usize, len: usize) -> bool {
        debug_assert!(page_aligned(offset));
        debug_assert!(page_aligned(len));
        let begin = self.addr + offset;
        let end = begin + len;
        if end > self.addr + self.size {
            return false;
        }
        // brute force
        for vmar in inner.children.iter() {
            if !(vmar.addr >= end || vmar.addr + vmar.size <= begin) {
                return false;
            }
        }
        for map in inner.mappings.iter() {
            if !(map.addr >= end || map.addr + map.size <= begin) {
                return false;
            }
        }
        true
    }
}

fn page_aligned(x: usize) -> bool {
    x % 0x1000 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_child() {
        let root_vmar = VmAddressRegion::new_root();
        let child = root_vmar
            .create_child(0, 0x2000)
            .expect("failed to create child VMAR");

        // test invalid argument
        assert_eq!(
            root_vmar.create_child(0x2001, 0x1000).err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            root_vmar.create_child(0x2000, 1).err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            root_vmar.create_child(0, 0x1000).err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            child.create_child(0x1000, 0x2000).err(),
            Some(ZxError::INVALID_ARGS)
        );
    }
}

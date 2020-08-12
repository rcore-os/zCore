use crate::error::*;
use crate::types::*;
use crate::sys;
use crate::pmem::{PMEM, PhysicalRegion};
use core::marker::PhantomData;
use crate::cap;

pub struct Object<T: ObjectBacking> {
    region: PhysicalRegion,
    object: CPtr,
    _phantom: PhantomData<T>,
}

pub unsafe trait ObjectBacking {
    fn bits() -> u8;
    unsafe fn retype(untyped: CPtr, out: CPtr) -> KernelResult<()>;
}

impl<T: ObjectBacking> Object<T> {
    pub fn new() -> KernelResult<Self> {
        let region = PMEM.alloc_region(T::bits())?;
        let object = match cap::G.allocate() {
            Ok(x) => x,
            Err(e) => {
                unsafe {
                    PMEM.release_region(region);
                }
                return Err(e);
            }
        };
        if let Err(e) = unsafe {
            T::retype(region.cap, object)
        } {
            panic!("Object::new: retype failed: {:?}", e);
        }
        Ok(Object { region, object, _phantom: PhantomData })
    }

    pub fn bits() -> u8 {
        T::bits()
    }
    
    pub fn region(&self) -> &PhysicalRegion {
        &self.region
    }

    pub fn object(&self) -> CPtr {
        self.object
    }
}

impl<T: ObjectBacking> Drop for Object<T> {
    fn drop(&mut self) {
        unsafe {
            sys::l4bridge_delete_cap_ts(self.object);
            cap::G.release(self.object);
            PMEM.release_region(self.region);
        }
    }
}
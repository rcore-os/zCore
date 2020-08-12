use crate::error::*;
use crate::types::*;
use crate::sys;
use crate::pmem::{PMEM, PhysicalRegion};
use core::marker::PhantomData;
use crate::cap;
use alloc::sync::Arc;
use core::ops::Deref;

struct ObjectStorage(PhysicalRegion);

impl Drop for ObjectStorage {
    fn drop(&mut self) {
        unsafe {
            PMEM.release_region(self.0);
        }
    }
}

pub struct Object<T: ObjectBacking> {
    region: Arc<ObjectStorage>,
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
        Ok(Object {
            region: Arc::new(ObjectStorage(region)),
            object,
            _phantom: PhantomData,
        })
    }

    pub fn bits() -> u8 {
        T::bits()
    }
    
    pub fn region(&self) -> &PhysicalRegion {
        &self.region.0
    }

    pub fn object(&self) -> CPtr {
        self.object
    }

    pub fn try_clone(&self) -> KernelResult<Self> {
        let cap = cap::G.allocate()?;
        if unsafe {
            sys::l4bridge_mint_cap_ts(self.object, cap, 0)
        } != 0 {
            panic!("Object::try_clone: cannot mint cap");
        }
        Ok(Self {
            region: self.region.clone(),
            object: cap,
            _phantom: PhantomData,
        })
    }
}

impl<T: ObjectBacking> Drop for Object<T> {
    fn drop(&mut self) {
        unsafe {
            sys::l4bridge_delete_cap_ts(self.object);
            cap::G.release(self.object);
        }
    }
}
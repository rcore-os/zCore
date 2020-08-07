use crate::{types::*, error::*};
use crate::sys;
use crate::cap;

pub struct Page {
    inner: CPtr,
    paddr: Word,
}

impl Page {
    pub fn allocate() -> KernelResult<Self> {
        let slot = cap::G.allocate()?;
        let mut paddr: Word = 0;
        match sys::locked(|| unsafe { sys::l4bridge_alloc_frame(slot, &mut paddr) }) {
            0 => Ok(Self {
                inner: slot,
                paddr,
            }),
            _ => {
                cap::G.release(slot);
                Err(KernelError::OutOfMemory)
            }
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        unsafe {
            sys::locked(|| sys::l4bridge_delete_cap(self.inner));
        }
        cap::G.release(self.inner);
    }
}
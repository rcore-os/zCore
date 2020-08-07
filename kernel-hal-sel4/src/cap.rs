use slab::Slab;
use crate::types::*;
use crate::sync::YieldMutex;
use crate::error::*;

const CAP_BASE: usize = 64;
const CAP_LIMIT: usize = 4096;

pub static G: CapAlloc = CapAlloc::new();

pub struct CapAlloc {
    slab: YieldMutex<Slab<()>>,
}

impl CapAlloc {
    pub const fn new() -> CapAlloc {
        CapAlloc {
            slab: YieldMutex::new(Slab::new()),
        }
    }

    pub fn allocate(&self) -> KernelResult<CPtr> {
        let mut slab = self.slab.lock();
        if slab.len() == CAP_LIMIT - CAP_BASE {
            Err(KernelError::OutOfCap)
        } else {
            let index = slab.insert(());
            Ok(CPtr(index + CAP_BASE))
        }
    }

    pub fn release(&self, cptr: CPtr) {
        self.slab.lock().remove(cptr.0 - CAP_BASE);
    }
}
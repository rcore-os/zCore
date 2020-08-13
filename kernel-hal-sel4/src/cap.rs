use slab::Slab;
use crate::types::*;
use crate::sync::YieldMutex;
use crate::error::*;
use crate::sys;
use crate::pmem::{PhysicalRegion, PMEM};
use crate::thread::yield_now;

const CAP_BASE: usize = 64;
const TOPLEVEL_BITS: u8 = 12;
const SECONDLEVEL_BITS: u8 = 12;
const TOPLEVEL_SIZE: usize = 1usize << TOPLEVEL_BITS;

pub static G: CapAlloc = CapAlloc::new();

pub struct CapAlloc {
    /// Used by `futexd` so we cannot use `FMutex` here.
    slab: YieldMutex<Slab<()>>,

    /// Used by `futexd` so we cannot use `FMutex` here.
    critical_buffer: YieldMutex<Option<PhysicalRegion>>,

    /// Used by `futexd` so we cannot use `FMutex` here.
    toplevel_usage: YieldMutex<[bool; TOPLEVEL_SIZE]>
}

#[derive(Copy, Clone, Debug)]
pub enum CriticalBufferUsage {
    Unused,
    Used,
}

impl CapAlloc {
    const fn new() -> CapAlloc {
        let mut toplevel_usage = [false; TOPLEVEL_SIZE];
        toplevel_usage[0] = true;
        CapAlloc {
            slab: YieldMutex::new(Slab::new()),
            critical_buffer: YieldMutex::new(None),
            toplevel_usage: YieldMutex::new(toplevel_usage),
        }
    }

    fn allocate_physical_region() -> KernelResult<PhysicalRegion> {
        let object_bits = SECONDLEVEL_BITS + unsafe { sys::L4BRIDGE_CNODE_SLOT_BITS as u8 };
        PMEM.alloc_region(object_bits)
    }

    fn ensure_cslot(&self, cptr: CPtr) -> KernelResult<CriticalBufferUsage> {
        let index = (cptr.0 >> SECONDLEVEL_BITS) & (TOPLEVEL_SIZE - 1);
        let mut toplevel_usage = self.toplevel_usage.lock();
        if !toplevel_usage[index] {
            let region = match self.critical_buffer.lock().take() {
                Some(x) => x,
                None => return Err(KernelError::Retry)
            };
            let err = sys::locked(|| unsafe { sys::l4bridge_retype_and_mount_cnode(
                region.cap,
                SECONDLEVEL_BITS as i32,
                index
            )});
            if err != 0 {
                panic!("ensure_cslot: cannot retype cnode");
            }
            toplevel_usage[index] = true;
            Ok(CriticalBufferUsage::Used)
        } else {
            Ok(CriticalBufferUsage::Unused)
        }
    }

    fn do_allocate(&self) -> KernelResult<(CPtr, CriticalBufferUsage)> {
        let mut slab = self.slab.lock();
        let index = slab.insert(());
        let cap = CPtr(index + CAP_BASE);
        let crit_buf_usage = self.ensure_cslot(cap)?;
        Ok((cap, crit_buf_usage))
    }

    pub fn allocate(&self) -> KernelResult<CPtr> {
        let (cptr, usage) = self.allocate_critical_mt()?;
        match usage {
            CriticalBufferUsage::Used => {
                self.refill_critical_buffer()?;
            }
            CriticalBufferUsage::Unused => {}
        }
        Ok(cptr)
    }

    pub fn allocate_critical_mt(&self) -> KernelResult<(CPtr, CriticalBufferUsage)> {
        // Retry until we successfully allocate a CPtr.
        //
        // This is required since when multiple threads call `allocate_critical_mt` really
        // fast, `refill_critical_buffer` may be needed from multiple threads but only called
        // on one of them. In that case we need to wait until it is actually called.
        for _ in 0..10000 {
            match self.do_allocate() {
                Ok(x) => return Ok(x),
                Err(KernelError::Retry) => {
                    yield_now();
                },
                Err(e) => return Err(e),
            }
        }
        panic!("allocate_critical_mt failed after many retries");
    }

    pub fn refill_critical_buffer(&self) -> KernelResult<()> {
        let mut buf = self.critical_buffer.lock();
        if buf.is_none() {
            *buf = Some(Self::allocate_physical_region()?);
        }
        Ok(())
    }

    pub fn release(&self, cptr: CPtr) {
        self.slab.lock().remove(cptr.0 - CAP_BASE);
    }
}

pub fn init() {
    G.refill_critical_buffer().expect("cap::init: cannot refill critical buffer");
}

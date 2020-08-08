use slab::Slab;
use crate::types::*;
use crate::sync::YieldMutex;
use crate::error::*;
use crate::sys;
use crate::pmem::{PhysicalRegion, PMEM};

const CAP_BASE: usize = 64;
const TOPLEVEL_BITS: u8 = 12;
const SECONDLEVEL_BITS: u8 = 12;
const TOPLEVEL_SIZE: usize = 1usize << TOPLEVEL_BITS;

pub static G: CapAlloc = CapAlloc::new();

pub struct CapAlloc {
    slab: YieldMutex<Slab<()>>,
    critical_buffer: YieldMutex<Option<PhysicalRegion>>,
    toplevel_usage: YieldMutex<[bool; TOPLEVEL_SIZE]>
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

    fn ensure_cslot(&self, cptr: CPtr, critical: bool) -> KernelResult<()> {
        let index = (cptr.0 >> SECONDLEVEL_BITS) & (TOPLEVEL_SIZE - 1);
        //println!("ensure_cslot {} {}", critical, index);
        let mut toplevel_usage = self.toplevel_usage.lock();
        if !toplevel_usage[index] {
            let region = if critical {
                println!("Taking critical region.");
                self.critical_buffer.lock().take().expect("ensure_cslot: empty critical buffer")
            } else {
                println!("Not taking critical region.");
                Self::allocate_physical_region()?
            };
            println!("Got region.");

            //println!("ensure_cslot 1");
            let err = sys::locked(|| unsafe { sys::l4bridge_retype_and_mount_cnode(
                region.cap,
                SECONDLEVEL_BITS as i32,
                index
            )});
            if err != 0 {
                panic!("ensure_cslot: cannot retype cnode");
            }
            toplevel_usage[index] = true;
            println!("CNode mounted.");

            //println!("ensure_cslot 2");
            Ok(())
        } else {

            //println!("ensure_cslot .");
            Ok(())
        }
    }

    pub fn do_allocate(&self, critical: bool) -> KernelResult<CPtr> {
        let mut slab = self.slab.lock();
        let index = slab.insert(());
        let cap = CPtr(index + CAP_BASE);
        drop(slab);
        self.ensure_cslot(cap, critical)?;
        Ok(cap)
    }

    pub fn allocate(&self) -> KernelResult<CPtr> {
        self.do_allocate(false)
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
    println!("sel4/cap: Initialized.");
}

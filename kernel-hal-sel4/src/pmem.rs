use crate::{types::*, error::*};
use crate::sys;
use crate::cap::{self, CriticalBufferUsage};
use alloc::collections::btree_map::BTreeMap;
use alloc::vec::Vec;
use crate::sync::YieldMutex;
use crate::object::*;
use core::sync::atomic::{AtomicUsize, Ordering};

pub static PMEM: PhysicalMemory = PhysicalMemory::new();

pub struct PhysicalMemory {
    /// Used by `futexd` so we cannot use `FMutex` here.
    regions: YieldMutex<BTreeMap<u8, Vec<PhysicalRegion>>>,

    /// Total size of our physical memory, in bytes.
    size: AtomicUsize,
}

#[derive(Copy, Clone, Debug)]
pub struct PhysicalRegion {
    pub cap: CPtr,
    pub paddr: Word,
    pub size_bits: u8,
}

impl PhysicalMemory {
    const fn new() -> PhysicalMemory {
        PhysicalMemory {
            regions: YieldMutex::new(BTreeMap::new()),
            size: AtomicUsize::new(0),
        }
    }

    fn init_collect_regions(&self) {
        let mut total_size: usize = 0;

        for bits in (16u8..=63u8).rev() {
            loop {
                let cslot = cap::G.allocate().expect("init_collect_regions: cannot allocate cap slot");
                let mut paddr: Word = 0;
                match sys::locked(|| unsafe { sys::l4bridge_alloc_untyped(cslot, bits as i32, &mut paddr)}) {
                    0 => {
                        total_size += 1 << bits;
                        self.regions.lock().entry(bits).or_insert(Vec::new()).push(PhysicalRegion {
                            cap: cslot,
                            paddr,
                            size_bits: bits,
                        });
                    },
                    _ => {
                        cap::G.release(cslot);
                        break;
                    }
                }
            }
        }

        println!("Total usable physical memory: {} bytes", total_size);
        self.size.store(total_size, Ordering::Relaxed);

        //println!("Regions: {:#x?}", *self.regions.lock());
    }

    pub fn size(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    pub fn alloc_region(&self, bits: u8) -> KernelResult<PhysicalRegion> {
        let mut regions = self.regions.lock();
        let mut critical_used = false;

        loop {
            match regions.range_mut(bits..).next() {
                Some((&min_bits, subregions)) => {
                    if min_bits == bits {
                        let subregion = subregions.pop().expect("alloc_region: no subregion");
                        if subregions.len() == 0 {
                            regions.remove(&min_bits);
                        }
                        drop(regions);
                        if critical_used {
                            cap::G.refill_critical_buffer().expect("alloc_region: out of memory for critical buffers");
                        }
                        break Ok(subregion);
                    } else {
                        let (dst_0, usage_0) = cap::G.allocate_critical_mt().expect("alloc_region: cannot allocate cap");
                        let (dst_1, usage_1) = cap::G.allocate_critical_mt().expect("alloc_region: cannot allocate cap");
                        match (usage_0, usage_1) {
                            (CriticalBufferUsage::Unused, CriticalBufferUsage::Unused) => {},
                            _ => {
                                critical_used = true;
                            }
                        }
                        let subregion = subregions.pop().expect("alloc_region: no subregion");
                        if subregions.len() == 0 {
                            regions.remove(&min_bits);
                        }

                        let err = sys::locked(|| unsafe { sys::l4bridge_split_untyped(subregion.cap, subregion.size_bits as i32, dst_0, dst_1) });
                        if err != 0 {
                            panic!("alloc_region: cannot split subregion");
                        }

                        let mut entry = regions.entry(subregion.size_bits - 1).or_insert(Vec::new());
                        entry.push(PhysicalRegion {
                            cap: dst_0,
                            paddr: subregion.paddr,
                            size_bits: subregion.size_bits - 1,
                        });
                        entry.push(PhysicalRegion {
                            cap: dst_1,
                            paddr: subregion.paddr + (1usize << (subregion.size_bits - 1)),
                            size_bits: subregion.size_bits - 1,
                        });
                    }
                }
                None => break Err(KernelError::OutOfMemory)
            }
        }
    }

    pub unsafe fn release_region(&self, region: PhysicalRegion) {
        self.regions.lock().entry(region.size_bits).or_insert(Vec::new()).push(region);
    }
}

pub fn init() {
    PMEM.init_collect_regions();
}

pub struct PageBacking;
unsafe impl ObjectBacking for PageBacking {
    fn bits() -> u8 {
        unsafe {
            sys::L4BRIDGE_PAGE_BITS as u8
        }
    }

    unsafe fn retype(untyped: CPtr, out: CPtr) -> KernelResult<()> {
        if sys::locked(|| sys::l4bridge_retype_page(untyped, out)) != 0 {
            Err(KernelError::RetypeFailed)
        } else {
            Ok(())
        }
    }
}

pub type Page = Object<PageBacking>;

impl Page {
    pub fn check_address_aligned(addr: usize) -> KernelResult<()> {
        if addr & ((1 << Self::bits()) - 1) != 0 {
            Err(KernelError::MisalignedAddress)
        } else {
            Ok(())
        }
    }
}

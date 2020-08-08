use crate::{types::*, error::*};
use crate::sys;
use crate::cap;
use alloc::collections::btree_map::BTreeMap;
use alloc::vec::Vec;
use crate::sync::YieldMutex;

pub static PMEM: PhysicalMemory = PhysicalMemory::new();

pub struct PhysicalMemory {
    regions: YieldMutex<BTreeMap<u8, Vec<PhysicalRegion>>>,
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
        }
    }

    fn init_collect_regions(&self) {
        for bits in (16u8..=63u8).rev() {
            loop {
                let cslot = cap::G.allocate().expect("init_collect_regions: cannot allocate cap slot");
                let mut paddr: Word = 0;
                match sys::locked(|| unsafe { sys::l4bridge_alloc_untyped(cslot, bits as i32, &mut paddr)}) {
                    0 => {
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

        //println!("Regions: {:#x?}", *self.regions.lock());
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
                            println!("Begin refill."); // DEADLOCKS HERE (re-entry)
                            cap::G.refill_critical_buffer().unwrap();
                            println!("End refill.");
                        }
                        break Ok(subregion);
                    } else {
                        //println!("case 2");
                        critical_used = true;
                        let dst_0 = cap::G.do_allocate(true).expect("alloc_region: cannot allocate cap");
                        let dst_1 = cap::G.do_allocate(true).expect("alloc_region: cannot allocate cap");
                        let subregion = subregions.pop().expect("alloc_region: no subregion");
                        if subregions.len() == 0 {
                            regions.remove(&min_bits);
                        }

                        //println!("case 2 - 1");

                        let err = sys::locked(|| unsafe { sys::l4bridge_split_untyped(subregion.cap, subregion.size_bits as i32, dst_0, dst_1) });
                        if err != 0 {
                            panic!("alloc_region: cannot split subregion");
                        }

                        //println!("case 2 - 2");

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

    fn release_region(&self, region: PhysicalRegion) {
        self.regions.lock().entry(region.size_bits).or_insert(Vec::new()).push(region);
    }
}

pub fn init() {
    PMEM.init_collect_regions();
}

pub struct Page {
    region: PhysicalRegion,
}

impl Page {
    pub fn allocate() -> KernelResult<Self> {
        let region = PMEM.alloc_region(12)?;
        Ok(Page { region })
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        PMEM.release_region(self.region);
    }
}

use alloc::collections::linked_list::LinkedList;
use crate::pmem::{PMEM, Page, PhysicalRegion};
use alloc::collections::btree_map::BTreeMap;
use core::ops::Range;
use crate::types::*;
use crate::error::*;
use crate::sys;
use lazy_static::lazy_static;
use crate::futex::FMutex;
use crate::cap;

lazy_static! {
    pub static ref K: FMutex<VmAlloc> = FMutex::new(
        unsafe { VmAlloc::with_vspace(CPtr(sys::L4BRIDGE_STATIC_CAP_VSPACE)) }
    );
}

pub struct VmAlloc {
    vspace: CPtr,
    paging_structures: LinkedList<PagingStructure>,
    pages: BTreeMap<usize, Page>,
    vm_regions: BTreeMap<usize, VmRegion>,
}

struct PagingStructure {
    region: PhysicalRegion,
    object: CPtr,
}

impl Drop for PagingStructure {
    fn drop(&mut self) {
        unsafe {
            sys::l4bridge_delete_cap_ts(self.object);
            cap::G.release(self.object);
            PMEM.release_region(self.region);
        }
    }
}

pub struct VmRegion {
    range: Range<usize>,
    readable: bool,
    writable: bool,
    executable: bool,
}

impl VmAlloc {
    pub const unsafe fn with_vspace(vspace: CPtr) -> Self {
        VmAlloc {
            vspace,
            paging_structures: LinkedList::new(),
            pages: BTreeMap::new(),
            vm_regions: BTreeMap::new(),
        }
    }

    fn map_page(&mut self, page: &Page, vaddr: usize) -> KernelResult<()> {
        let vspace = self.vspace;
        unsafe {
            map_level(|| {
                match sys::locked(|| sys::l4bridge_map_page(
                    page.object(), vspace, vaddr, 0
                )) {
                    0 => Ok(()),
                    _ => Err(KernelError::MissingPagingParents),
                }
            }, || {
                let level = prepare_level(sys::L4BRIDGE_PAGETABLE_BITS as u8, sys::l4bridge_retype_pagetable)?;
                let object = level.object;
                self.paging_structures.push_back(level);
                map_level(|| match sys::locked(|| sys::l4bridge_map_pagetable(
                    object, vspace, vaddr
                )) {
                    0 => Ok(()),
                    _ => Err(KernelError::MissingPagingParents)
                }, || {
                    let level = prepare_level(sys::L4BRIDGE_PAGEDIR_BITS as u8, sys::l4bridge_retype_pagedir)?;
                    let object = level.object;
                    self.paging_structures.push_back(level);
                    map_level(|| match sys::locked(|| sys::l4bridge_map_pagedir(
                        object, vspace, vaddr
                    )) {
                        0 => Ok(()),
                        _ => Err(KernelError::MissingPagingParents)
                    }, || {
                        let level = prepare_level(sys::L4BRIDGE_PDPT_BITS as u8, sys::l4bridge_retype_pdpt)?;
                        let object = level.object;
                        self.paging_structures.push_back(level);
                        match sys::locked(|| sys::l4bridge_map_pdpt(
                            object, vspace, vaddr
                        )) {
                            0 => Ok(()),
                            _ => Err(KernelError::MissingPagingParents)
                        }
                    })
                })
            })
        }
    }

    pub fn release_region(&mut self, vaddr: usize) {
        let region = self.vm_regions.range((vaddr..)).next();
        if let Some((index, region)) = region {
            if vaddr < region.range.end {
                for addr in (region.range.start..region.range.end).step_by(1 << Page::bits()) {
                    if self.pages.remove(&addr).is_none() {
                        panic!("VmAlloc::release_region: cannot free page");
                    }
                }
                let index = *index;
                self.vm_regions.remove(&index);
                return;
            }
        }
        panic!("VmAlloc::release_region: cannot find region");
    }

    pub fn vframe_addr(vaddr: usize) -> usize {
        vaddr & (!((1 << Page::bits()) - 1))
    }

    pub fn vframe_offset(vaddr: usize) -> usize {
        vaddr & ((1 << Page::bits()) - 1)
    }

    pub fn page_at(&self, vaddr: usize) -> Option<&Page> {
        let vaddr = Self::vframe_addr(vaddr);
        self.pages.get(&vaddr)
    }

    pub fn insert_page(&mut self, addr: usize, page: Page) -> KernelResult<()> {
        Page::check_address_aligned(addr)?;
        if self.pages.contains_key(&addr) {
            return Err(KernelError::VmRegionOverlap);
        }
        self.map_page(&page, addr)?;
        self.pages.insert(addr, page);
        Ok(())
    }

    /// This function doesn't return the removed page because we unmap the page
    /// by dropping it.
    pub fn remove_page(&mut self, vaddr: usize) -> bool {
        return self.pages.remove(&vaddr).is_some();
    }

    pub fn allocate_region(&mut self, range: Range<usize>) -> KernelResult<()> {
        // Requirement 1: Range is properly aligned
        Page::check_address_aligned(range.start)?;
        Page::check_address_aligned(range.end)?;

        // Requirement 2: No overlap
        for addr in (range.start..range.end).step_by(1 << Page::bits()) {
            if self.pages.contains_key(&addr) {
                return Err(KernelError::VmRegionOverlap);
            }
        }

        // Try to allocate pages
        let mut pages: LinkedList<(usize, Page)> = LinkedList::new();
        for addr in (range.start..range.end).step_by(1 << Page::bits()) {
            let page = Page::new()?;
            self.map_page(&page, addr)?;
            pages.push_back((addr, page));
        }
        for (addr, page) in pages {
            self.pages.insert(addr, page);
        }

        self.vm_regions.insert(range.start, VmRegion {
            range,
            readable: true,
            writable: true,
            executable: true,
        });
        Ok(())
    }
}

fn map_level<F: FnMut() -> KernelResult<()>, G: FnOnce() -> KernelResult<()>>(mut this_level: F, outer_level: G) -> KernelResult<()> {
    let res = this_level();
    if res.is_err() {
        let res = outer_level();
        if res.is_err() {
            res
        } else {
            this_level()
        }
    } else {
        res
    }
}

unsafe fn prepare_level(size_bits: u8, retyper: unsafe extern "C" fn (CPtr, CPtr) -> i32) -> KernelResult<PagingStructure> {
    let region = PMEM.alloc_region(size_bits)?;
    let object = match cap::G.allocate() {
        Ok(x) => x,
        Err(e) => {
            unsafe {
                PMEM.release_region(region);
            }
            return Err(e);
        }
    };
    if sys::locked(|| retyper(region.cap, object)) != 0 {
        panic!("prepare_level: cannot retype object");
    }
    Ok(PagingStructure {
        region,
        object,
    })
}

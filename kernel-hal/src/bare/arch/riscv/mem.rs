use alloc::vec::Vec;
use core::ops::Range;

use crate::addr::{align_down, align_up};
use crate::utils::init_once::InitOnce;
use crate::{PhysAddr, KCONFIG, PAGE_SIZE};

pub fn free_pmem_regions() -> Vec<Range<PhysAddr>> {
    extern "C" {
        fn end();
    }

    static FREE_PMEM_REGIONS: InitOnce<Vec<Range<PhysAddr>>> = InitOnce::new();
    FREE_PMEM_REGIONS.init_once(|| {
        let initrd = super::INITRD_REGION.as_ref();
        let min_start = align_up(end as usize + PAGE_SIZE) - KCONFIG.phys_to_virt_offset;

        let mut regions = Vec::new();
        for r in super::MEMORY_REGIONS.iter() {
            let (start, end) = (align_up(r.start.max(min_start)), align_down(r.end));
            if start >= end {
                continue;
            }
            if let Some(initrd) = initrd {
                // no overlap at all
                if initrd.end <= start || initrd.start >= end {
                    regions.push(start..end);
                    continue;
                }
                // no overlap on the left
                if initrd.start > start {
                    regions.push(start..align_down(initrd.start));
                }
                // no overlap on the right
                if initrd.end < end {
                    regions.push(align_up(initrd.end)..end);
                }
            } else {
                regions.push(start..end);
            }
        }
        regions
    });
    FREE_PMEM_REGIONS.clone()
}

pub fn frame_flush(_target: crate::PhysAddr) {
    unimplemented!()
}

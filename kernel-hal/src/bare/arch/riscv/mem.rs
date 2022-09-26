use alloc::vec::Vec;
use core::ops::Range;

use crate::addr::{align_down, align_up};
use crate::utils::init_once::InitOnce;
use crate::{PhysAddr, KCONFIG, PAGE_SIZE};

fn cut_off(total: Range<PhysAddr>, cut: &Range<PhysAddr>) -> Vec<Range<PhysAddr>> {
    let mut regions = Vec::new();
    // no overlap at all
    if cut.end <= total.start || total.end <= cut.start {
        regions.push(total);
    } else {
        // no overlap on the left
        if total.start < cut.start {
            regions.push(total.start..align_down(cut.start));
        }
        // no overlap on the right
        if cut.end < total.end {
            regions.push(align_up(cut.end)..total.end);
        }
    }
    regions
}

pub fn free_pmem_regions() -> Vec<Range<PhysAddr>> {
    extern "C" {
        fn end();
    }

    static FREE_PMEM_REGIONS: InitOnce<Vec<Range<PhysAddr>>> = InitOnce::new();
    FREE_PMEM_REGIONS.init_once(|| {
        let initrd = super::INITRD_REGION.as_ref();
        let dtb = Range::<PhysAddr> {
            start: KCONFIG.dtb_paddr,
            end: KCONFIG.dtb_paddr + KCONFIG.dtb_size,
        };
        let min_start = align_up(end as usize + PAGE_SIZE) - KCONFIG.phys_to_virt_offset;

        let mut regions = Vec::new();
        for r in super::MEMORY_REGIONS.iter() {
            let base = align_up(r.start.max(min_start))..align_down(r.end);
            let mut no_dtb = cut_off(base, &dtb);
            if let Some(initrd) = initrd {
                for range in no_dtb {
                    let mut no_initrd = cut_off(range, initrd);
                    regions.append(&mut no_initrd);
                }
            } else {
                regions.append(&mut no_dtb);
            }
        }
        regions
    });
    FREE_PMEM_REGIONS.clone()
}

pub fn frame_flush(_target: crate::PhysAddr) {
    unimplemented!()
}

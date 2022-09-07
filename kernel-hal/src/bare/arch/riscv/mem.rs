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
            // limit max memory & reserve memory for dtb
            cfg_if! {
                if #[cfg(feature = "board_fu740")] {
                    let (mut start, mut end) = (align_up(r.start.max(min_start)), align_down(r.end));
                    end = end.min(0xFFFF_F000);
                    let dtb_start = crate::KCONFIG.dtb_paddr;
                    let dtb_end = dtb_start + 20000;
                    // overlap on the left
                    if dtb_start <= start && dtb_end <= end {
                        start = align_up(dtb_end);
                    }
                    // overlap on the right
                    else if start <= dtb_start && end <= dtb_end {
                        end = align_down(dtb_start);
                    }
                    // overlap on the middle
                    else if start < dtb_start && dtb_end < end {
                        let end_2 = end;
                        end = align_down(dtb_start);
                        let start_2 = align_up(dtb_end);
                        // push (start_2, end_2)
                        if let Some(initrd) = initrd {
                            // no overlap at all
                            if initrd.end <= start_2 || initrd.start >= end_2 {
                                regions.push(start_2..end_2);
                                continue;
                            }
                            // no overlap on the left
                            if initrd.start > start_2 {
                                regions.push(start_2..align_down(initrd.start));
                            }
                            // no overlap on the right
                            if initrd.end < end_2 {
                                regions.push(align_up(initrd.end)..end_2);
                            }
                        } else {
                            regions.push(start_2..end_2);
                        }
                    }
                    // do nothing if no overlap at all
                } else {
                    let (start, end) = (align_up(r.start.max(min_start)), align_down(r.end.min(0xFFFF_F000)));
                }
            }
            if start >= end {
                continue;
            }
            // reserve memory for initrd
            if let Some(initrd) = initrd {
                // no overlap at all
                if initrd.end <= start || end <= initrd.start {
                    regions.push(start..end);
                    continue;
                }
                // no overlap on the left
                if start < initrd.start {
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

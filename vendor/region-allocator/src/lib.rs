#![no_std]

extern crate alloc;

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::cmp::{max, min};

#[derive(Eq, Copy, Clone, Debug, Ord, PartialEq, PartialOrd)]
struct Region {
    base: usize,
    size: usize,
}

/// An endpoint-based region allocator.
#[derive(Default)]
pub struct RegionAllocator {
    regions: BTreeSet<Region>,
}

impl RegionAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, base: usize, size: usize) {
        let mut new_region = Region { base, size };
        for region in self.intersection_all(&new_region) {
            if let Some(region) = Self::merge_internal(&mut new_region, region) {
                self.regions.insert(region);
            }
        }
        self.regions.insert(new_region);
    }

    pub fn subtract(&mut self, base: usize, size: usize) {
        let mut new_region = Region { base, size };
        for region in self.intersection_all(&new_region) {
            let (left, right) = Self::subtract_internal(region, &mut new_region);
            self.regions.extend(left);
            self.regions.extend(right);
        }
    }

    pub fn add_or_subtract(&mut self, base: usize, size: usize, is_add: bool) {
        if is_add {
            self.add(base, size);
        } else {
            self.subtract(base, size);
        }
    }

    pub fn allocate_by_addr(&mut self, base: usize, size: usize) -> bool {
        if self
            .regions
            .iter()
            .any(|region| region.base <= base && base + size <= region.base + region.size)
        {
            self.subtract(base, size);
            true
        } else {
            false
        }
    }

    pub fn allocate_by_size(&mut self, size: usize, alignment: usize) -> Option<(usize, usize)> {
        if !alignment.is_power_of_two() {
            return None;
        }
        let align = alignment - 1;
        let base = self.regions.iter().find_map(|region| {
            if size > region.size {
                return None;
            }
            let base = (region.base + align) & !align;
            (region.base <= base && base + size <= region.base + region.size).then_some(base)
        })?;
        self.subtract(base, size);
        Some((base, size))
    }

    pub fn check_region(&self, base: usize, size: usize) -> bool {
        self.regions.contains(&Region { base, size })
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn check_point(&self, addr: usize) -> bool {
        self.regions
            .iter()
            .any(|region| region.base <= addr && addr <= region.base + region.size)
    }

    fn intersection_all(&mut self, region: &Region) -> Vec<Region> {
        self.regions
            .extract_if(.., |candidate| {
                !(candidate.base > region.base + region.size
                    || candidate.base + candidate.size < region.base)
            })
            .collect()
    }

    fn merge_internal(target: &mut Region, other: Region) -> Option<Region> {
        let target_end = target.base + target.size;
        let other_end = other.base + other.size;
        if target_end < other.base || other_end < target.base {
            return Some(other);
        }
        let new_base = min(target.base, other.base);
        let new_end = max(target_end, other_end);
        target.base = new_base;
        target.size = new_end - new_base;
        None
    }

    fn subtract_internal(target: Region, source: &mut Region) -> (Option<Region>, Option<Region>) {
        let target_end = target.base + target.size;
        let source_end = source.base + source.size;
        let left = (source.base > target.base).then(|| Region {
            base: target.base,
            size: min(target.size, source.base - target.base),
        });
        let right = (source_end < target_end).then(|| {
            let size = min(target.size, target_end - source_end);
            Region {
                base: target_end - size,
                size,
            }
        });
        (left, right)
    }
}

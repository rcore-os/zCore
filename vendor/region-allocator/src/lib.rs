#![no_std]

extern crate alloc;

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::cmp::{max, min};

#[derive(Eq, Copy, Clone, Debug, Ord, PartialEq, PartialOrd)]
struct Region {
    pub base: usize,
    pub size: usize,
}

/// An endpoint-based region allocator.
#[derive(Default)]
pub struct RegionAllocator {
    regions: BTreeSet<Region>,
}

impl RegionAllocator {
    /// Create an empty [`RegionAllocator`].
    pub fn new() -> Self {
        RegionAllocator::default()
    }
    /// Add a region `[base, base + size)` to the set.
    /// The left endpoint is inclusive, and the right endpoint is exclusive.
    ///
    /// Any two overlapped or adjacent regions will be merged.
    /// In the final region set, no regions are intersected.
    /// For example if both `[0, 10)` and `[10, 20)` are added sequentially,
    /// only `[0, 20)` will be in the final region set.
    pub fn add(&mut self, base: usize, size: usize) {
        let mut new_region = Region { base, size };
        let overlaps = self.intersection_all(&new_region);
        for b in overlaps {
            if let Some(b) = Self::merge_internal(&mut new_region, b) {
                self.insert_internal(b);
            }
        }
        self.insert_internal(new_region);
    }
    /// Subtract the whole region set with a given region.
    /// After this operation, all regions in the set have no intersection with the given one.
    /// Regions completely contained by the given region will be removed.
    /// Regions wholly containing the given region will be splitted into two parts
    pub fn subtract(&mut self, base: usize, size: usize) {
        let mut new_region = Region { base, size };
        let overlaps = self.intersection_all(&new_region);
        for b in overlaps {
            let res = Self::subtract_internal(b, &mut new_region);
            if let Some(b) = res.0 {
                self.insert_internal(b);
            }
            if let Some(b) = res.1 {
                self.insert_internal(b);
            }
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
        for r in &self.regions {
            if r.base <= base && base + size <= r.base + r.size {
                self.subtract(base, size);
                return true;
            }
        }
        false
    }
    /// Allocate a region at an arbitrary position aligned to a given power of 2.
    pub fn allocate_by_size(&mut self, size: usize, alignment: usize) -> Option<(usize, usize)> {
        if !alignment.is_power_of_two() {
            return None;
        }
        let align = alignment - 1;
        for r in &self.regions {
            if size > r.size {
                continue;
            }
            let base = (r.base + align) & !align;
            if r.base <= base && base + size <= r.base + r.size {
                self.subtract(base, size);
                return Some((base, size));
            }
        }
        None
    }
    /// Find if any region perfectly match a given range.
    pub fn check_region(&self, base: usize, size: usize) -> bool {
        self.regions.contains(&Region { base, size })
    }
    /// Return number of regions in the set.
    pub fn len(&self) -> usize {
        self.regions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
    /// Check whether the point is covered.
    pub fn check_point(&self, addr: usize) -> bool {
        for r in &self.regions {
            if r.base <= addr && addr <= r.base + r.size {
                return true;
            }
        }
        false
    }

    fn intersection_all(&mut self, region: &Region) -> Vec<Region> {
        // `BTreeSet::drain_filter` ha sido inestable y no es fiable entre nightlies.
        // Hacemos el equivalente de forma portable: recolectar -> eliminar -> devolver.
        let overlaps: Vec<Region> = self
            .regions
            .iter()
            .copied()
            .filter(|r| !(r.base > region.base + region.size || r.base + r.size < region.base))
            .collect();
        for r in &overlaps {
            self.regions.remove(r);
        }
        overlaps
    }
    fn insert_internal(&mut self, a: Region) {
        self.regions.insert(a);
    }
    fn merge_internal(a: &mut Region, b: Region) -> Option<Region> {
        let a_end = a.base + a.size;
        let b_end = b.base + b.size;
        if a_end < b.base || b_end < a.base {
            return Some(b);
        }
        let new_base = min(a.base, b.base);
        let new_end = max(a_end, b_end);
        let new_size = new_end - new_base;
        a.base = new_base;
        a.size = new_size;
        None
    }
    fn subtract_internal(target: Region, src: &mut Region) -> (Option<Region>, Option<Region>) {
        let t_end = target.base + target.size;
        let s_end = src.base + src.size;
        let left = if src.base > target.base {
            Some(Region {
                base: target.base,
                size: min(target.size, src.base - target.base),
            })
        } else {
            None
        };
        let right = if s_end < t_end {
            let size = min(target.size, t_end - s_end);
            Some(Region {
                base: t_end - size,
                size,
            })
        } else {
            None
        };
        (left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::RegionAllocator;

    #[test]
    fn add_test_2() {
        let mut alloc = RegionAllocator::new();
        alloc.add(0, 500);
        alloc.add(600, 100);
        assert!(alloc.check_region(0, 500));
        assert!(alloc.check_region(600, 100));
        alloc.add(500, 100);
        assert!(alloc.check_region(0, 700));
    }
    #[test]
    fn add_test() {
        let mut alloc = RegionAllocator::new();
        // Case 1: not intersecting
        alloc.add(0, 100);
        alloc.add(200, 300);
        alloc.add(600, 100);
        assert!(alloc.check_region(0, 100));
        assert!(alloc.check_region(200, 300));
        assert!(alloc.check_region(600, 100));
        // Case 2: extending tail
        alloc.add(100, 50);
        assert!(!alloc.check_region(0, 100));
        assert!(alloc.check_region(0, 150));
        assert!(alloc.check_region(200, 300));
        assert!(alloc.check_region(600, 100));
        alloc.add(100, 60);
        assert!(alloc.check_region(0, 160));
        // Case 3: extending head
        alloc.add(180, 20);
        assert!(alloc.check_region(180, 320));
        alloc.add(165, 60);
        assert!(alloc.check_region(165, 335));
        assert!(alloc.check_region(600, 100));
        // Case 4: merging two blocks
        alloc.add(160, 5);
        assert!(alloc.check_region(0, 500));
        assert!(alloc.check_region(600, 100));
        alloc.add(500, 100);
        assert!(alloc.check_region(0, 700));
    }
    #[test]
    fn sub_test() {
        let mut alloc = RegionAllocator::new();
        alloc.add(0, 100);
        alloc.add(200, 300);
        alloc.add(600, 100);
        // Case 1: not intersecting
        alloc.subtract(500, 100);
        assert_eq!(alloc.len(), 3);
        // Case 2: trimming head
        alloc.subtract(500, 150);
        assert!(alloc.check_region(650, 50));
        assert!(!alloc.check_region(600, 50));
        // Case 3: trimming tail
        alloc.subtract(680, 44);
        assert!(alloc.check_region(650, 30));
        // Case 4: removing a whole
        alloc.subtract(500, 300);
        assert!(!alloc.check_region(650, 50));
        assert_eq!(alloc.len(), 2);
        // Case 5: trimming both head and tail
        alloc.subtract(50, 200);
        assert!(alloc.check_region(0, 50));
        assert!(alloc.check_region(250, 250));
        assert_eq!(alloc.len(), 2);
        // Case 6: cut in the middle
        alloc.subtract(300, 100);
        assert!(alloc.check_region(250, 50));
        assert!(alloc.check_region(400, 100));
        assert_eq!(alloc.len(), 3);
    }
    #[test]
    fn alloc_test() {
        let mut alloc = RegionAllocator::new();
        alloc.add(0, 100);
        alloc.add(200, 300);
        alloc.add(600, 200);
        // Case 1: successful alloc
        assert_eq!(alloc.allocate_by_addr(10, 10), true);
        assert_eq!(alloc.allocate_by_size(12, 1 << 3), Some((24, 12)));
        // Case 2: invalid args
        assert_eq!(alloc.allocate_by_size(1, 9), None);
        // Case 3: unsuccessful alloc
        assert_eq!(alloc.allocate_by_addr(0, 20), false);
        assert_eq!(alloc.allocate_by_addr(30, 20), false);
        assert_eq!(alloc.allocate_by_size(400, 1), None);
        assert_eq!(alloc.allocate_by_size(300, 1 << 5), None);
        // Change regions and alloc again
        alloc.add(500, 100);
        assert_eq!(alloc.allocate_by_size(400, 1 << 6), Some((256, 400)));
    }
}

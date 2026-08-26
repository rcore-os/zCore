//! Definition of phyical, virtual addresses and helper functions.

use crate::PAGE_SIZE;

/// Physical address.
pub type PhysAddr = usize;

/// Virtual address.
pub type VirtAddr = usize;

/// Device address.
pub type DevVAddr = usize;

pub const fn align_down(addr: usize) -> usize {
    addr & !(PAGE_SIZE - 1)
}

pub const fn align_up(addr: usize) -> usize {
    (addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

pub const fn is_aligned(addr: usize) -> bool {
    page_offset(addr) == 0
}

pub const fn page_count(size: usize) -> usize {
    align_up(size) / PAGE_SIZE
}

pub const fn page_offset(addr: usize) -> usize {
    addr & (PAGE_SIZE - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The whole module is built on a 4 KiB page; lock that assumption down so
    // the rest of the expectations below stay meaningful.
    #[test]
    fn page_size_is_4k() {
        assert_eq!(PAGE_SIZE, 4096);
        // PAGE_SIZE must be a power of two for the bit-mask tricks to work.
        assert!(PAGE_SIZE.is_power_of_two());
    }

    #[test]
    fn align_down_basic() {
        assert_eq!(align_down(0), 0);
        assert_eq!(align_down(1), 0);
        assert_eq!(align_down(PAGE_SIZE - 1), 0);
        assert_eq!(align_down(PAGE_SIZE), PAGE_SIZE);
        assert_eq!(align_down(PAGE_SIZE + 1), PAGE_SIZE);
        assert_eq!(align_down(0x1234), 0x1000);
        assert_eq!(align_down(0x2fff), 0x2000);
    }

    #[test]
    fn align_down_is_idempotent() {
        for &a in &[0usize, 1, 0x999, 0x1000, 0x1abc, 0x10_0000 + 7] {
            let once = align_down(a);
            assert_eq!(once, align_down(once));
            assert!(is_aligned(once));
            // Never rounds up.
            assert!(once <= a);
            // Stays within one page below the input.
            assert!(a - once < PAGE_SIZE);
        }
    }

    #[test]
    fn align_up_basic() {
        assert_eq!(align_up(0), 0);
        assert_eq!(align_up(1), PAGE_SIZE);
        assert_eq!(align_up(PAGE_SIZE - 1), PAGE_SIZE);
        assert_eq!(align_up(PAGE_SIZE), PAGE_SIZE);
        assert_eq!(align_up(PAGE_SIZE + 1), 2 * PAGE_SIZE);
        assert_eq!(align_up(0x1234), 0x2000);
        assert_eq!(align_up(0x2000), 0x2000);
    }

    #[test]
    fn align_up_is_idempotent_and_never_rounds_down() {
        for &a in &[0usize, 1, 0x999, 0x1000, 0x1abc, 0x10_0000 + 7] {
            let once = align_up(a);
            assert_eq!(once, align_up(once));
            assert!(is_aligned(once));
            assert!(once >= a);
            assert!(once - a < PAGE_SIZE);
        }
    }

    #[test]
    fn align_up_down_relationship() {
        // For an already-aligned address the two agree; otherwise they bracket
        // the address exactly one page apart.
        for &a in &[0x1000usize, 0x2000, 0x10_0000] {
            assert_eq!(align_down(a), align_up(a));
        }
        for &a in &[1usize, 0x999, 0x1001, 0x2abc] {
            assert_eq!(align_up(a) - align_down(a), PAGE_SIZE);
        }
    }

    #[test]
    fn is_aligned_basic() {
        assert!(is_aligned(0));
        assert!(is_aligned(PAGE_SIZE));
        assert!(is_aligned(4 * PAGE_SIZE));
        assert!(!is_aligned(1));
        assert!(!is_aligned(PAGE_SIZE - 1));
        assert!(!is_aligned(PAGE_SIZE + 1));
        assert!(!is_aligned(0x1234));
    }

    #[test]
    fn page_offset_basic() {
        assert_eq!(page_offset(0), 0);
        assert_eq!(page_offset(1), 1);
        assert_eq!(page_offset(PAGE_SIZE), 0);
        assert_eq!(page_offset(PAGE_SIZE + 7), 7);
        assert_eq!(page_offset(0x1234), 0x234);
        // offset + aligned base reconstructs the original address.
        for &a in &[0usize, 1, 0x1234, 0x2fff, 0x10_0007] {
            assert_eq!(align_down(a) + page_offset(a), a);
        }
    }

    #[test]
    fn page_count_basic() {
        assert_eq!(page_count(0), 0);
        assert_eq!(page_count(1), 1);
        assert_eq!(page_count(PAGE_SIZE), 1);
        assert_eq!(page_count(PAGE_SIZE + 1), 2);
        assert_eq!(page_count(2 * PAGE_SIZE), 2);
        assert_eq!(page_count(2 * PAGE_SIZE - 1), 2);
        assert_eq!(page_count(10 * PAGE_SIZE), 10);
    }

    #[test]
    fn page_count_matches_align_up() {
        for &size in &[0usize, 1, 0x999, 0x1000, 0x1001, 0x12_3456] {
            assert_eq!(page_count(size) * PAGE_SIZE, align_up(size));
        }
    }
}

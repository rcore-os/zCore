// aarch64

use spin::Once;

static OFFSET: Once<usize> = Once::new();

#[inline]
pub(super) fn save_offset(offset: usize) {
    OFFSET.call_once(|| offset);
}

#[inline]
pub fn phys_to_virt_offset() -> usize {
    *OFFSET.wait()
}

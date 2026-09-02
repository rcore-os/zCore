//! I/O port permissions.

use core::ops::{Deref, DerefMut};
use x86_64::registers::model_specific::GsBase;
use x86_64::structures::tss::TaskStateSegment;

/// TSS with port bitmap, allocated consecutively.
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub(super) struct TSSWithPortBitmap {
    tss: TaskStateSegment,
    /// Bit in port_bitmap: 0 indicates accessible, 1 indicated inaccessible.
    /// Follow linux, add one extra element.
    port_bitmap: [u8; 1 + Self::BITMAP_VALID_SIZE],
}

impl Deref for TSSWithPortBitmap {
    type Target = TaskStateSegment;

    fn deref(&self) -> &Self::Target {
        &self.tss
    }
}

impl DerefMut for TSSWithPortBitmap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tss
    }
}

impl TSSWithPortBitmap {
    const BITMAP_VALID_SIZE: usize = u16::MAX as usize / 8;

    /// Create a new TSS with port bitmap.
    ///
    /// All ioports are denied by default.
    pub fn new() -> Self {
        const DENY_ALL: u8 = !0;
        let mut tss = Self {
            tss: TaskStateSegment::new(),
            port_bitmap: [DENY_ALL; 1 + Self::BITMAP_VALID_SIZE],
        };
        tss.iomap_base = core::mem::size_of::<TaskStateSegment>() as u16;
        tss
    }
}

/// Get ioport bitmap.
pub fn bitmap() -> &'static mut [u8] {
    unsafe {
        let gsbase = GsBase::MSR.read();
        let tss = &mut *(gsbase as *mut TSSWithPortBitmap);
        &mut tss.port_bitmap[..]
    }
}

/// Get ioport permission.
///
/// Return true for allow, false for deny.
pub fn get_permission(port: u16) -> bool {
    let bitmap = bitmap();
    let idx: usize = (port >> 3) as usize;
    let bit: u8 = (port & 0x7) as u8;
    bitmap[idx] & (1 << bit) == 0
}

/// Set ioport permission.
pub fn set_permission(port: u16, allow: bool) {
    let bitmap = bitmap();
    let idx: usize = (port >> 3) as usize;
    let bit: u8 = (port & 0x7) as u8;
    let deny: u8 = if allow { 0 } else { 1 };
    bitmap[idx] &= !(1 << bit);
    bitmap[idx] |= deny << bit;
}

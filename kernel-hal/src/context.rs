use core::fmt;

#[repr(C, align(16))]
#[derive(Debug, Copy, Clone)]
pub struct VectorRegs {
    pub fcw: u16,
    pub fsw: u16,
    pub ftw: u8,
    pub _pad0: u8,
    pub fop: u16,
    pub fip: u32,
    pub fcs: u16,
    pub _pad1: u16,

    pub fdp: u32,
    pub fds: u16,
    pub _pad2: u16,
    pub mxcsr: u32,
    pub mxcsr_mask: u32,

    pub mm: [U128; 8],
    pub xmm: [U128; 16],
    pub reserved: [U128; 3],
    pub available: [U128; 3],
}

// https://xem.github.io/minix86/manual/intel-x86-and-64-manual-vol1/o_7281d5ea06a5b67a-274.html
impl Default for VectorRegs {
    fn default() -> Self {
        VectorRegs {
            fcw: 0x37f,
            mxcsr: 0x1f80,
            ..unsafe { core::mem::zeroed() }
        }
    }
}

// workaround: libcore has bug on Debug print u128 ??
#[derive(Default, Clone, Copy)]
#[repr(C, align(16))]
pub struct U128(pub [u64; 2]);

impl fmt::Debug for U128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#016x}{:016x}", self.0[1], self.0[0])
    }
}

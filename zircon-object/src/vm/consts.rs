#[cfg(not(target_arch = "mips"))]
pub const KERNEL_VMAR_BASE: usize = 0xffff_ff02_0000_0000;
#[cfg(not(target_arch = "mips"))]
pub const KERNEL_VMAR_SIZE: usize = 0x8000_00000;
#[cfg(not(target_arch = "mips"))]
pub const ROOT_VMAR_ADDR: usize = 0x2_00000000;
#[cfg(not(target_arch = "mips"))]
pub const ROOT_VMAR_SIZE: usize = 0x100_00000000;

#[cfg(target_arch = "mips")]
pub const KERNEL_VMAR_BASE: usize = 0x80100000;
#[cfg(target_arch = "mips")]
pub const KERNEL_VMAR_SIZE: usize = 0x4_00000;
#[cfg(target_arch = "mips")]
pub const ROOT_VMAR_ADDR: usize = 0x100000;
#[cfg(target_arch = "mips")]
pub const ROOT_VMAR_SIZE: usize = 0x8000000;

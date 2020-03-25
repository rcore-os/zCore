/// This struct contains constants that are initialized by the kernel
/// once at boot time.  From the vDSO code's perspective, they are
/// read-only data that can never change.  Hence, no synchronization is
/// required to read them.
#[repr(C)]
pub struct VdsoConstants {
    /// Maximum number of CPUs that might be online during the lifetime
    /// of the booted system.
    max_num_cpus: u32,
    /// Bit map indicating features.
    features: Features,
    /// Number of bytes in a data cache line.
    dcache_line_size: u32,
    /// Number of bytes in an instruction cache line.
    icache_line_size: u32,
    /// Conversion factor for zx_ticks_get return values to seconds.
    ticks_per_second: u64,
    ticks_to_mono_numerator: u32,
    ticks_to_mono_denominator: u32,
    /// Total amount of physical memory in the system, in bytes.
    physmem: u64,
    /// A build id of the system. Currently a non-null terminated ascii
    /// representation of a git SHA.
    buildid: [u8; MAX_BUILDID_SIZE],
}

#[allow(unsafe_code)]
pub fn generate_constants() -> (usize, [u8; 112]) {
    let constants = VdsoConstants {
        max_num_cpus: 1,
        features: Features {
            cpu: 0,
            hw_breakpoint_count: 0,
            hw_watchpoint_count: 0,
        },
        dcache_line_size: 0,
        icache_line_size: 0,
        ticks_per_second: 30_0000_0000u64,
        ticks_to_mono_numerator: 1000u32,
        ticks_to_mono_denominator: 3000u32,
        physmem: 0u64,
        buildid: [0u8; MAX_BUILDID_SIZE],
    };
    (VDSO_CONSTANT_BASE,unsafe { core::mem::transmute(constants) })
}

/// Bit map indicating features.
///
/// For specific feature bits, see zircon/features.h.
#[repr(C)]
#[derive(Debug)]
struct Features {
    cpu: u32,
    /// Total amount of debug registers available in the system.
    hw_breakpoint_count: u32,
    hw_watchpoint_count: u32,
}

const MAX_BUILDID_SIZE: usize = 63;
const VDSO_CONSTANT_BASE: usize = 0x4940;

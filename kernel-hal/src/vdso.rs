use core::fmt::{Debug, Error, Formatter};

/// This struct contains constants that are initialized by the kernel
/// once at boot time.  From the vDSO code's perspective, they are
/// read-only data that can never change.  Hence, no synchronization is
/// required to read them.
#[repr(C)]
#[derive(Debug)]
pub struct VdsoConstants {
    /// Maximum number of CPUs that might be online during the lifetime
    /// of the booted system.
    pub max_num_cpus: u32,
    /// Bit map indicating features.
    pub features: Features,
    /// Number of bytes in a data cache line.
    pub dcache_line_size: u32,
    /// Number of bytes in an instruction cache line.
    pub icache_line_size: u32,
    /// Conversion factor for zx_ticks_get return values to seconds.
    pub ticks_per_second: u64,
    /// Ratio which relates ticks (zx_ticks_get) to clock monotonic.
    ///
    /// Specifically: ClockMono(ticks) = (ticks * N) / D
    pub ticks_to_mono_numerator: u32,
    pub ticks_to_mono_denominator: u32,
    /// Total amount of physical memory in the system, in bytes.
    pub physmem: u64,
    /// Actual length of `version_string`, not including the NUL terminator.
    pub version_string_len: u64,
    /// A NUL-terminated UTF-8 string returned by `zx_system_get_version_string`.
    pub version_string: VersionString,
}

/// Bit map indicating features.
///
/// For specific feature bits, see zircon/features.h.
#[repr(C)]
#[derive(Debug)]
pub struct Features {
    pub cpu: u32,
    /// Total amount of debug registers available in the system.
    pub hw_breakpoint_count: u32,
    pub hw_watchpoint_count: u32,
}

impl VdsoConstants {
    /// Set version string.
    pub fn set_version_string(&mut self, s: &str) {
        let len = s.len().min(64);
        self.version_string_len = len as u64;
        self.version_string.0[..len].copy_from_slice(s.as_bytes());
    }
}

#[repr(C)]
pub struct VersionString([u8; 64]);

impl Default for VersionString {
    fn default() -> Self {
        VersionString([0; 64])
    }
}

impl Debug for VersionString {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        for &c in self.0.iter().take_while(|&&c| c != 0) {
            write!(f, "{}", c as char)?;
        }
        Ok(())
    }
}

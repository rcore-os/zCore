use core::fmt::{Debug, Error, Formatter};
use git_version::git_version;

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
    /// A build id of the system. Currently a non-null terminated ascii
    /// representation of a git SHA.
    pub buildid: BuildID,
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

#[repr(C)]
pub struct BuildID([u8; 64]);

impl Default for BuildID {
    fn default() -> Self {
        let s = git_version!(
            prefix = "git-",
            args = ["--always", "--abbrev=40", "--dirty=-dirty"]
        );
        let len = s.len().min(64);
        let mut bytes = [0; 64];
        bytes[..len].copy_from_slice(s.as_bytes());
        BuildID(bytes)
    }
}

impl Debug for BuildID {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        for &c in self.0.iter().take_while(|&&c| c != 0) {
            write!(f, "{}", c as char)?;
        }
        Ok(())
    }
}

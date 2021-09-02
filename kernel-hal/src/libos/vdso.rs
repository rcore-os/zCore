use git_version::git_version;

use super::mem_common::PMEM_SIZE;

pub use crate::common::vdso::*;

pub fn vdso_constants() -> VdsoConstants {
    let tsc_frequency = 3000u16;
    let mut constants = VdsoConstants {
        max_num_cpus: 1,
        features: Features {
            cpu: 0,
            hw_breakpoint_count: 0,
            hw_watchpoint_count: 0,
        },
        dcache_line_size: 0,
        icache_line_size: 0,
        ticks_per_second: tsc_frequency as u64 * 1_000_000,
        ticks_to_mono_numerator: 1000,
        ticks_to_mono_denominator: tsc_frequency as u32,
        physmem: PMEM_SIZE as u64,
        version_string_len: 0,
        version_string: Default::default(),
    };
    constants.set_version_string(git_version!(
        prefix = "git-",
        args = ["--always", "--abbrev=40", "--dirty=-dirty"]
    ));
    constants
}

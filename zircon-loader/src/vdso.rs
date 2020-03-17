use {
    alloc::{sync::Arc, vec::Vec},
    spin::Mutex,
    zircon_object::{object::*, vm::*},
};
/// This struct contains constants that are initialized by the kernel
/// once at boot time.  From the vDSO code's perspective, they are
/// read-only data that can never change.  Hence, no synchronization is
/// required to read them.
#[allow(dead_code)]
#[repr(C)]
//#[derive(Debug)]
struct VdsoConstants {
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
    /// Total amount of physical memory in the system, in bytes.
    physmem: u64,
    /// A build id of the system. Currently a non-null terminated ascii
    /// representation of a git SHA.
    buildid: [u8; MAX_BUILDID_SIZE],
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

const MAX_BUILDID_SIZE: usize = 64;
pub const VDSO_VARIANT_COUNT: usize = 3;
const VDSO_NAMES: [&str; VDSO_VARIANT_COUNT] = ["vdso/full", "vdso/test1", "vdso/test2"];

lazy_static! {
    pub static ref VDSO_VMOS: Mutex<VDsos> = Mutex::new(VDsos {
        vmos: {
            let vmo = VmObject::new(VMObjectPaged::new(0));
            vec![vmo.clone(); VDSO_VARIANT_COUNT]
        },
    });
}

pub struct VDsos {
    vmos: Vec<Arc<VmObject>>,
}

impl VDsos {
    pub fn init(&mut self, vdso_vmo: Arc<dyn VMObjectTrait>) {
        self.vmos[0] = VmObject::new(vdso_vmo.clone());
        self.vmos[0].set_name(VDSO_NAMES[0]);
        for i in 1..VDSO_VARIANT_COUNT {
            self.vmos[i] = VmObject::new(vdso_vmo.create_clone(0, vdso_vmo.len()));
            self.vmos[i].set_name(VDSO_NAMES[i]);
        }
    }

    pub fn get_vdso_handles(&self, handles: &mut [Handle]) {
        assert_eq!(handles.len(), VDSO_VARIANT_COUNT);
        for (i, vmo) in self.vmos.iter().enumerate() {
            handles[i] = Handle::new(vmo.clone(), Rights::DEFAULT_VMO | Rights::EXECUTE);
        }
    }
}

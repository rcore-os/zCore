use crate::types::*;
use crate::error::*;
use crate::pmem::Page;
use crate::vm;
pub use kernel_hal::*;
pub use kernel_hal::defs::*;
use core::fmt::Debug;
use core::time::Duration;
use kernel_hal::vdso::{Features, VdsoConstants};
use git_version::git_version;
use crate::kt;
use crate::control;
use alloc::boxed::Box;
use trapframe::UserContext;
use crate::thread::LocalContext;
use crate::user::{UserThread, KernelEntryReason};

const PHYS_IDMAP_BASE: usize = 0x6000_0000_0000;

#[repr(C)]
pub struct PhysFrame {
    vpaddr: usize,
}

impl Debug for PhysFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
        write!(f, "PhysFrame({:#x})", self.vpaddr)
    }
}

impl PhysFrame {
    #[export_name = "hal_frame_alloc"]
    pub extern "C" fn alloc() -> Option<Self> {
        let page = match Page::new() {
            Ok(x) => x,
            Err(e) => {
                println!("hal_frame_alloc: allocation failed");
                return None;
            }
        };
        let vpaddr = PHYS_IDMAP_BASE + page.region().paddr;
        match vm::K.lock().insert_page(vpaddr, page) {
            Ok(_) => {}
            Err(e) => {
                println!("hal_frame_alloc: insert failed");
                return None;
            }
        }
        Some(PhysFrame {
            vpaddr,
        })
    }

    #[export_name = "hal_zero_frame_paddr"]
    pub fn zero_frame_addr() -> PhysAddr {
        0
    }

    pub fn addr(&self) -> PhysAddr {
        self.vpaddr
    }
}

impl Drop for PhysFrame {
    #[export_name = "hal_frame_dealloc"]
    fn drop(&mut self) {
        assert_eq!(vm::K.lock().remove_page(self.vpaddr), true);
    }
}

/// Read physical memory from `paddr` to `buf`.
#[export_name = "hal_pmem_read"]
pub fn pmem_read(paddr: PhysAddr, buf: &mut [u8]) {
    assert!(paddr >= PHYS_IDMAP_BASE);
    unsafe {
        (paddr as *const u8).copy_to_nonoverlapping(buf.as_mut_ptr(), buf.len());
    }
}

/// Copy content of `src` frame to `target` frame
#[export_name = "hal_frame_copy"]
pub fn frame_copy(src: PhysAddr, target: PhysAddr) {
    assert!(src >= PHYS_IDMAP_BASE);
    assert!(target >= PHYS_IDMAP_BASE);
    unsafe {
        (src as *const u8).copy_to_nonoverlapping(target as *mut u8, PAGE_SIZE);
    }
}

/// Zero `target` frame.
#[export_name = "hal_frame_zero"]
pub fn frame_zero(target: PhysAddr) {
    assert!(target >= PHYS_IDMAP_BASE);
    unsafe {
        core::ptr::write_bytes(target as *mut u8, 0, PAGE_SIZE);
    }
}

/// Flush the physical frame.
#[export_name = "hal_frame_flush"]
pub fn frame_flush(_target: PhysAddr) {
    // do nothing
}

/// Get current time.
#[export_name = "hal_timer_now"]
pub fn timer_now() -> Duration {
    Duration::from_nanos(crate::timer::now())
}

#[export_name = "hal_context_run"]
unsafe fn context_run(context: &mut UserContext) {
    // Our seL4 layer supports N:M kt/ut scheduling but it seems that zCore
    // expects 1:1 kt/ut mapping.
    let user_thread = LocalContext::current().user_thread.borrow_mut()
        .take().expect("context_run: no user task");
    let old_ut_ptr: *const UserThread = &*user_thread;
    let (entry_reason, user_thread) = user_thread.run(context);
    let new_ut_ptr: *const UserThread = &*user_thread;
    assert_eq!(old_ut_ptr, new_ut_ptr);
    *LocalContext::current().user_thread.borrow_mut() = Some(user_thread);

    match entry_reason {
        KernelEntryReason::UnknownSyscall => {
            // TODO: usermode vdso injection for register swapping
            context.general.rdx = context.general.rax;
            context.trap_num = 0x100; // emulate x86-64 hardware trap number
        }
        _ => {
            // TODO: handle this
            context.trap_num = 0x0;
        }
    }
}

/// Set a new timer.
///
/// After `deadline`, the `callback` will be called.
#[export_name = "hal_timer_set"]
pub fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
    kt::spawn(move || {
        let now = timer_now();
        if deadline > now {
            control::sleep((deadline - now).as_nanos() as u64);
        }
        callback(timer_now());
    }).expect("timer_set: cannot spawn thread");
}

#[export_name = "hal_vdso_constants"]
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
        physmem: 1048576 * 128,
        version_string_len: 0,
        version_string: Default::default(),
    };
    constants.set_version_string(git_version!(
        prefix = "git-",
        args = ["--always", "--abbrev=40", "--dirty=-dirty"]
    ));
    constants
}
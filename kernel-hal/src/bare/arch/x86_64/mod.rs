mod drivers;
#[cfg(feature = "graphic")]
mod early_fb_console;
// `bare::timer::timer_tick` reaches `power::thermal_governor_tick` from outside
// this arch module, so the governor needs crate-wide visibility.
pub(crate) mod power;
// `vm.rs` consults `pat::pat_wc_ready` when emitting WriteCombining PTEs.
pub(crate) mod pat;
mod smp;
mod trap;

pub mod config;
pub mod cpu;
pub mod interrupt;
pub mod mem;
pub mod timer;
pub mod vm;

#[doc(cfg(target_arch = "x86_64"))]
pub mod special;

hal_fn_impl! {
    impl mod crate::hal_fn::console {
        fn console_write_early(_s: &str) {
            #[cfg(feature = "graphic")]
            {
                // Note: this is within the kernel-hal crate, so we can reference the private
                // `imp::arch` module.
                crate::imp::arch::early_fb_console::write_str(_s);
            }
        }

        fn console_progress_early(_progress: u32) {
            #[cfg(feature = "graphic")]
            {
                crate::imp::arch::early_fb_console::draw_progress_bar(_progress);
            }
        }

        fn console_panic_banner(_s: &str) {
            #[cfg(feature = "graphic")]
            {
                crate::imp::arch::early_fb_console::panic_banner(_s);
            }
        }
    }
}

use crate::{mem::phys_to_virt, KCONFIG};
use x86_64::registers::control::{Cr4, Cr4Flags};

pub const fn timer_interrupt_vector() -> usize {
    trap::X86_INT_APIC_TIMER
}

pub fn cmdline() -> alloc::string::String {
    // `boot_info.cmdline` carries a *physical* pointer (rboot runs identity-mapped,
    // like `initrd_start`). Reading the `&str` directly only works while rboot's
    // identity map is still live; once the kernel switches to its own page tables
    // that low address is unmapped and the read faults (observed as a kernel page
    // fault on a ~physical address during boot). Read it through the physmap,
    // which always maps all RAM, so it is valid regardless of the identity map.
    let s = KCONFIG.cmdline;
    let virt = phys_to_virt(s.as_ptr() as usize);
    let bytes = unsafe { core::slice::from_raw_parts(virt as *const u8, s.len()) };
    alloc::string::String::from_utf8_lossy(bytes).into_owned()
}

pub fn init_ram_disk() -> Option<&'static mut [u8]> {
    if KCONFIG.initrd_start == 0 || KCONFIG.initrd_size == 0 {
        return None;
    }
    let start = phys_to_virt(KCONFIG.initrd_start as usize);
    Some(unsafe { core::slice::from_raw_parts_mut(start as *mut u8, KCONFIG.initrd_size as usize) })
}

pub fn primary_init_early() {
    // init serial output first
    drivers::init_early().unwrap();
}

pub fn primary_init() {
    // Give this CPU a write-combining PAT entry and retype the framebuffer's
    // physmap PTEs to it BEFORE the display drivers come up, so the graphic
    // console never pushes a frame through uncached stores. See `pat.rs`.
    pat::init_this_cpu();
    pat::enable_framebuffer_wc();
    drivers::init().unwrap();
    warn!("[boot] drivers init complete");
    unsafe {
        // enable global page
        Cr4::update(|f| f.insert(Cr4Flags::PAGE_GLOBAL));
    }
    // Scale the BSP's P-state and pick its idle C-state before bringing up the
    // APs (each AP runs the same per-CPU setup from `secondary_init`). This is
    // what keeps the CPU from running hot at idle on real hardware.
    power::init();
    smp::start_application_processors();
    warn!("[boot] smp init complete");
}

pub fn timer_init() {
    timer::init();
}

pub fn secondary_init() {
    // Defense in depth: re-assert CR0.WP=1 on this AP. The trampoline already
    // sets it, but this invariant is load-bearing (a WP=0 AP silently writes
    // through read-only user/shared pages from kernel mode — e.g. a
    // copy_to_user onto the shared ZERO_FRAME poisons every demand-zero page,
    // crashing apk's mimalloc), so we also guarantee it here where it is
    // discoverable in Rust, surviving any future trampoline refactor.
    unsafe {
        core::arch::asm!(
            "mov {tmp}, cr0",
            "or  {tmp}, {wp}",
            "mov cr0, {tmp}",
            tmp = out(reg) _,
            wp = const 1u64 << 16,
            options(nostack, preserves_flags),
        );
    }
    // The PAT MSR is per-core and must agree with the BSP's (which already
    // redefined entry 7 to WC and retyped the framebuffer PTEs to use it)
    // before this AP touches any WC mapping.
    pat::init_this_cpu();
    zcore_drivers::irq::x86::Apic::init_local_apic_ap();
    // The LAPIC timer's mode/divide/initial-count registers are per-CPU and are
    // only programmed on the BSP (in `drivers.rs`). Replicate that here so this
    // AP actually receives the 250 Hz scheduler tick; without it the AP's timer
    // stays stopped (initial count 0) and the core never preempts or idles
    // cleanly. The unmask itself happens later via `timer_enable()`.
    timer::program_periodic_tick();
    // P-state/idle setup is per-logical-CPU, so every AP must run it too.
    power::init();
    smp::ap_signal_online();
}

/// Dense logical CPU id for the AP currently starting (trampoline slot).
pub fn ap_trampoline_logical_id() -> u8 {
    smp::ap_trampoline_logical_id()
}

/// Release the BSP to reuse the shared trampoline slots: called by the starting
/// AP the instant it has latched its logical id out of the slot.
pub fn ap_signal_slot_consumed() {
    smp::ap_signal_slot_consumed();
}

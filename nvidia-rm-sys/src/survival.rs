//! GPU-independent survival channel for the console-GPU GSP-boot wedge.
//!
//! The console GPU wedges the CPU on a posted BAR1/fabric write ~1/3 of the
//! time during the SEC2 HS-resume window. When it does, nothing survives to
//! tell us *where*: the `/proc` report needs the machine alive to finish, and
//! the framebuffer is the very GPU we just wedged. With no serial port on this
//! box we need a breadcrumb that outlives a full CPU hang and a cold reboot and
//! never touches the GPU/PCIe.
//!
//! The RTC/CMOS NVRAM (I/O ports 0x70 index / 0x71 data) is exactly that: a
//! few battery-backed bytes reachable with two `out`/`in` instructions,
//! completely independent of the GPU, that keep their value across a hang and a
//! power cycle. We record a coarse **milestone** byte and a fine rolling
//! **narration counter** (bumped on every RM `nv_printf` line) as the boot
//! advances. After a wedge, `cat /proc/gpusurvive` on the next (healthy) boot
//! reads them back, so a single reboot tells us the exact operation the
//! previous attempt died on — e.g. milestone `STARTCPU_PRE` with narration
//! count N means it hung on the STARTCPU store after N RM narration lines.
//!
//! Bytes it *would* use: 0x40 magic, 0x41 milestone, 0x42 narration counter.
//!
//! WRITES ARE NOW DISABLED. On real hardware (ASUS PRIME X299-A II / AMI BIOS)
//! those bytes turned out to be INSIDE the firmware's checksummed NVRAM region
//! — not above it as the classic PC map suggested — so every write broke the
//! BIOS settings checksum and the next boot halted at POST with "Please enter
//! setup to recover BIOS setting / Press F1", reverting settings to defaults.
//! The breadcrumb was already superseded by the live console trace, so
//! `cmos_write` is a no-op; the GPU bring-up must never scribble on CMOS.
//! Reads are harmless and kept, so `/proc/gpusurvive` still compiles (it now
//! just reports "no breadcrumb").

use core::sync::atomic::{AtomicBool, Ordering};

const CMOS_MAGIC_OFF: u8 = 0x40;
const CMOS_MILESTONE_OFF: u8 = 0x41;
const CMOS_NARR_OFF: u8 = 0x42;
/// Distinguishes "Eclipse wrote this" from random battery-backed garbage.
const MAGIC: u8 = 0xEC;

/// Coarse milestones written to CMOS as the console-GPU GSP boot advances. The
/// value that survives a wedge names the last operation reached.
pub mod milestone {
    pub const NONE: u8 = 0x00;
    /// gsp_boot_run entered for the console GPU.
    pub const BOOT_ENTER: u8 = 0x10;
    /// PBUS PRI error retired; about to call into the vendored kgspInitRm.
    pub const INITRM_CALL: u8 = 0x20;
    /// os_boundary is about to issue the posted STARTCPU store (the wedge point).
    pub const STARTCPU_PRE: u8 = 0x40;
    /// The posted STARTCPU store returned (fabric did NOT wedge on it).
    pub const STARTCPU_POST: u8 = 0x50;
    /// PDISP restore ran after the SEC2 window (only if a quiesce was armed).
    pub const PDISP_RESTORE: u8 = 0x60;
    /// kgspInitRm returned (OK or a clean NV_STATUS error — not a wedge).
    pub const INITRM_RETURN: u8 = 0x70;
    /// bringup_step14: RM API controls stage reached.
    pub const CONTROLS: u8 = 0x80;
    /// bringup_step14: gpuStatePreInit/Init/Load stage reached.
    pub const STATE_LOAD: u8 = 0x90;
    /// bringup_step14: copy-engine data-movement stage reached.
    pub const CE_MOVE: u8 = 0xA0;
    /// Full console bring-up chain completed.
    pub const COMPLETE: u8 = 0xFF;
}

/// Human label for a milestone byte (for the /proc report).
pub fn milestone_label(m: u8) -> &'static str {
    match m {
        milestone::NONE => "none (no prior attempt recorded)",
        milestone::BOOT_ENTER => "BOOT_ENTER (console gsp_boot_run entered)",
        milestone::INITRM_CALL => "INITRM_CALL (PBUS cleared, about to call kgspInitRm)",
        milestone::STARTCPU_PRE => "STARTCPU_PRE (WEDGED ON the posted STARTCPU store)",
        milestone::STARTCPU_POST => "STARTCPU_POST (STARTCPU store completed; wedged later)",
        milestone::PDISP_RESTORE => "PDISP_RESTORE (past the SEC2 window)",
        milestone::INITRM_RETURN => "INITRM_RETURN (kgspInitRm returned)",
        milestone::CONTROLS => "CONTROLS (RM API controls stage)",
        milestone::STATE_LOAD => "STATE_LOAD (gpuState*Init/Load stage)",
        milestone::CE_MOVE => "CE_MOVE (copy-engine data movement stage)",
        milestone::COMPLETE => "COMPLETE (full chain finished)",
        _ => "unknown",
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val,
        options(nomem, nostack, preserves_flags));
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", out("al") val, in("dx") port,
        options(nomem, nostack, preserves_flags));
    val
}

/// Read one CMOS byte. Bit 7 of the index port disables NMI for the duration of
/// the access (the standard idiom); we re-select register 0x0D (read-only,
/// harmless) with NMI re-enabled afterwards so we never leave NMI masked.
#[cfg(target_arch = "x86_64")]
unsafe fn cmos_read(idx: u8) -> u8 {
    outb(0x70, 0x80 | (idx & 0x7f));
    let v = inb(0x71);
    outb(0x70, 0x0d);
    let _ = inb(0x71);
    v
}

/// DISABLED. Writing these extended-CMOS bytes (0x40-0x42) corrupted the BIOS
/// settings checksum on real hardware (ASUS PRIME X299-A II / AMI BIOS): the
/// next boot halted at POST with "Please enter setup to recover BIOS setting --
/// Press F1 to Run SETUP" and the firmware reverted to defaults. Contrary to
/// the classic PC CMOS map, this firmware's checksummed NVRAM region DOES cover
/// 0x40-0x42, so any write there breaks the checksum. The survival breadcrumb
/// was already superseded by the live console trace, so the write is simply a
/// no-op now -- the GPU bring-up must never touch CMOS. Reads stay (harmless;
/// they only toggle NMI and never alter settings), so callers keep compiling
/// and `/proc/gpusurvive` just reports "no breadcrumb".
#[cfg(target_arch = "x86_64")]
unsafe fn cmos_write(_idx: u8, _val: u8) {}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn cmos_read(_idx: u8) -> u8 {
    0
}
#[cfg(not(target_arch = "x86_64"))]
unsafe fn cmos_write(_idx: u8, _val: u8) {}

/// Record a coarse milestone (and stamp the magic so a later read knows the
/// bytes are ours). Safe to call from anywhere, including the STARTCPU bracket
/// with interrupts off — it is two port writes and touches no lock, no GPU.
pub fn checkpoint(m: u8) {
    unsafe {
        cmos_write(CMOS_MAGIC_OFF, MAGIC);
        cmos_write(CMOS_MILESTONE_OFF, m);
    }
}

/// Bump the rolling narration counter (wraps at 256). Called on every RM
/// nv_printf line so a wedge's surviving count pinpoints how far the RM's own
/// narration got before it died.
pub fn narration_tick() {
    unsafe {
        let n = cmos_read(CMOS_NARR_OFF);
        cmos_write(CMOS_NARR_OFF, n.wrapping_add(1));
    }
}

/// Zero the narration counter at the start of a fresh attempt.
pub fn reset_narration() {
    unsafe { cmos_write(CMOS_NARR_OFF, 0) };
}

// --- Console-GPU MSI accounting (shared drivers <-> os_boundary) ---------
// The console GSP boot brings the GPU's MSI delivery online (drivers side); the
// STARTCPU bracket (os_boundary) logs the state right before the posted store,
// so a wedge's frozen screen shows whether MSI was actually online and how many
// fired — the datum that scrolls off the top otherwise.
use core::sync::atomic::AtomicUsize;
static MSI_ONLINE_VECTOR: AtomicUsize = AtomicUsize::new(usize::MAX);
static MSI_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Mark the GPU's MSI delivery online for the boot (vector), zeroing the count.
pub fn msi_set_online(vector: usize) {
    MSI_COUNT.store(0, Ordering::Relaxed);
    MSI_ONLINE_VECTOR.store(vector, Ordering::Relaxed);
}
/// Mark MSI delivery offline (boot done / never came online).
pub fn msi_offline() {
    MSI_ONLINE_VECTOR.store(usize::MAX, Ordering::Relaxed);
}
/// One MSI serviced (called from the ISR closure). Returns the new count.
pub fn msi_tick() -> usize {
    MSI_COUNT.fetch_add(1, Ordering::Relaxed) + 1
}
/// (online-vector | usize::MAX, count) for the pre-STARTCPU status line.
pub fn msi_status() -> (usize, usize) {
    (
        MSI_ONLINE_VECTOR.load(Ordering::Relaxed),
        MSI_COUNT.load(Ordering::Relaxed),
    )
}

static REPORTED: AtomicBool = AtomicBool::new(false);

/// Read back the breadcrumb the previous attempt left, format a one-block
/// report, then clear it (idempotent within a boot: repeated calls after the
/// first return "already read this boot"). Call from `/proc/gpusurvive`.
pub fn read_report_and_clear() -> alloc::string::String {
    use alloc::string::String;
    use core::fmt::Write;
    let mut s = String::new();
    let (magic, milestone, narr) = unsafe {
        (
            cmos_read(CMOS_MAGIC_OFF),
            cmos_read(CMOS_MILESTONE_OFF),
            cmos_read(CMOS_NARR_OFF),
        )
    };
    if magic != MAGIC {
        let _ = writeln!(
            s,
            "[gpusurvive] no console-GPU breadcrumb recorded (CMOS magic {:#04x} != {:#04x}) -- either no attempt ran since the last clear, or the firmware reused the bytes.",
            magic, MAGIC
        );
        return s;
    }
    let _ = writeln!(
        s,
        "[gpusurvive] previous console-GPU boot attempt breadcrumb:"
    );
    let _ = writeln!(
        s,
        "[gpusurvive]   last milestone = {:#04x}  ({})",
        milestone,
        milestone_label(milestone)
    );
    let _ = writeln!(
        s,
        "[gpusurvive]   RM narration lines emitted before the freeze = {}",
        narr
    );
    if milestone == milestone::STARTCPU_PRE {
        let _ = writeln!(
            s,
            "[gpusurvive]   => WEDGED on the posted STARTCPU store, as the fabric-backpressure model predicts."
        );
    } else if milestone >= milestone::INITRM_RETURN {
        let _ = writeln!(
            s,
            "[gpusurvive]   => got past kgspInitRm; any freeze was in a LATER stage, not the SEC2 window."
        );
    }
    // Clear so the next attempt starts from a known-empty slate.
    unsafe {
        cmos_write(CMOS_MAGIC_OFF, 0);
        cmos_write(CMOS_MILESTONE_OFF, milestone::NONE);
        cmos_write(CMOS_NARR_OFF, 0);
    }
    if REPORTED.swap(true, Ordering::Relaxed) {
        s.push_str(
            "[gpusurvive]   (note: breadcrumb already read once this boot; values now cleared)\n",
        );
    }
    s
}

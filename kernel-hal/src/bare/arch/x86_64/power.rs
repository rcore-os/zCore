//! x86_64 CPU power management: P-state scaling (Intel HWP / EPB, AMD CPPC) and
//! a deeper idle entry (MONITOR/MWAIT).
//!
//! ## Why this module exists
//!
//! On real hardware an otherwise-idle Eclipse OS ran hot (~80 °C). The
//! scheduler already halts the CPU correctly when it runs out of work (see
//! [`super::interrupt`]'s `wait_for_interrupt`), which is exactly why **under
//! QEMU the host CPU use stays low** — the guest really does execute a halt and
//! the vCPU thread sleeps. The heat is bare-metal-only.
//!
//! It comes from two things the kernel never did:
//!
//! 1. **No P-state control.** With no OS-directed power management the firmware
//!    leaves each core at a high fixed performance point (often the maximum
//!    non-turbo ratio, sometimes turbo). High core voltage/frequency burns
//!    power — and therefore generates heat — even at 0 % load.
//! 2. **Shallow idle.** A plain `hlt` only reaches C1, which gates the core
//!    clock but keeps the voltage up.
//!
//! QEMU exposes neither HWP/CPPC nor real C-states to the guest and does not
//! model the silicon's voltage/frequency/thermals (the host OS governs the
//! physical CPU), so neither problem is observable there — both bite only on
//! physical silicon. This module addresses both, once per logical CPU at
//! bring-up:
//!
//! * **Hardware-autonomous P-states.** On Intel via HWP ("Speed Shift"), on AMD
//!   via CPPC. Enable the feature and let the CPU scale on its own between its
//!   lowest P-state and a ceiling, biased toward *performance* (EPP) so it ramps
//!   promptly to the ceiling under load. The ceiling defaults to the highest
//!   (turbo/boost) clock — an idle core still settles at its lowest
//!   voltage/frequency, and sustained-heat control is the adaptive thermal
//!   governor further down, which walks the ceiling down only when the package
//!   actually gets hot; the steady state takes no per-tick MSR pokes. `noturbo`
//!   on the cmdline caps the ceiling at the base (guaranteed/nominal) clock
//!   instead, the old open-loop heat lever for poorly cooled machines.
//! * **Energy-Performance Bias.** The legacy pre-HWP Intel hint (Sandy Bridge …
//!   Broadwell, and HWP parts that lack the EPP field) nudges the package's
//!   internal P-state and turbo decisions; kept in step with the EPP preference.
//! * **MWAIT idle.** Where supported, park in C1E via MONITOR/MWAIT instead of
//!   `hlt`, shedding a little more idle voltage. C1/C1E never stop the LAPIC
//!   timer, so this stays correct with the kernel's tickless-idle scheduler
//!   tick (which relies on the LAPIC timer to wake an idle CPU).
//!
//! Everything is gated on CPUID and the CPU vendor, and is **bare-metal only**:
//! under any hypervisor the host owns the physical power state, so the whole
//! module no-ops. That keeps the already-correct QEMU idle behaviour untouched
//! and avoids a #GP on a VMM that advertises a feature in CPUID but does not
//! implement the backing MSR.

use core::arch::x86_64::__cpuid;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use lock::Mutex;
use x86_64::registers::model_specific::Msr;

// ── Intel HWP / EPB MSRs ────────────────────────────────────────────────────
/// IA32_PM_ENABLE — bit 0 turns HWP on (a write-once latch, sticky to reset).
const IA32_PM_ENABLE: u32 = 0x770;
/// IA32_HWP_CAPABILITIES — read-only performance bounds of this part.
const IA32_HWP_CAPABILITIES: u32 = 0x771;
/// IA32_HWP_REQUEST — the OS's per-logical-CPU P-state request to the hardware.
const IA32_HWP_REQUEST: u32 = 0x774;
/// IA32_ENERGY_PERF_BIAS — legacy 4-bit efficiency hint (pre-HWP).
const IA32_ENERGY_PERF_BIAS: u32 = 0x1B0;
/// IA32_PM_ENABLE.HWP_ENABLE.
const HWP_ENABLE: u64 = 1 << 0;

// ── AMD CPPC MSRs (modelled on Linux's `amd_pstate` EPP driver) ──────────────
/// MSR_AMD_CPPC_CAP1 — read-only performance bounds (Highest[31:24] … Lowest[7:0]).
const MSR_AMD_CPPC_CAP1: u32 = 0xC001_02B0;
/// MSR_AMD_CPPC_ENABLE — bit 0 turns CPPC on.
const MSR_AMD_CPPC_ENABLE: u32 = 0xC001_02B1;
/// MSR_AMD_CPPC_REQUEST — Max[7:0] | Min[15:8] | Desired[23:16] | EPP[31:24].
const MSR_AMD_CPPC_REQUEST: u32 = 0xC001_02B2;

// ── Tunables ────────────────────────────────────────────────────────────────
//
// This kernel lets the CPU scale autonomously up to its turbo ceiling under
// load (base clock only with `noturbo`) and drop to the lowest P-state at
// idle, with the adaptive thermal governor (below) as the closed-loop cooling
// mechanism. Earlier this
// preference was biased to *maximum power saving* (EPP 0xFF), which pins a
// bursty, mostly-idle workload near the *lowest* P-state — ~3-4x slower than
// base on a modern part — so boot and every program launch crawled on real
// hardware. (This module is skipped under a hypervisor, so QEMU stayed fast and
// hid it — matching the "fast in QEMU, very slow on USB" report.) Bias to
// performance instead: ramp promptly to the ceiling under load, and let the
// thermal governor — not a permanently throttled clock — handle heat.
//
// Energy-Performance Preference written into the HWP/CPPC request [31:24]:
//   0x00 = maximum performance … 0xFF = maximum power saving.
// To trade speed back for a cooler package, raise `EPP_PREF` toward 0x80
// (balanced) or 0xFF (max saving); to shed turbo heat without a rebuild, boot
// with `noturbo` (the governor still walks the ceiling down when hot).
const EPP_PREF: u64 = 0x00;

// IA32_ENERGY_PERF_BIAS[3:0]: 0 = performance … 15 = power saving (max).
// Kept in step with EPP_PREF; only consulted on older parts without HWP-EPP.
const EPB_PREF: u64 = 0;

// Whether to cap the maximum P-state at the CPU's *guaranteed* (base) clock
// instead of its *highest* (turbo) clock. Turbo/boost bins run at the highest
// voltage and frequency and are the largest heat source — capping at base was
// the original open-loop heat lever, from before the adaptive thermal governor
// below existed. With the governor closing the loop (it walks the ceiling down
// as the package nears TjMax and back up as it cools), leaving turbo on is safe
// and is worth 20-30% single-thread throughput on a typical desktop part, so
// turbo is now the default. `noturbo` (or `turbo=off`) on the kernel cmdline
// restores the old base-clock cap for machines with inadequate cooling.
fn cap_at_base_clock() -> bool {
    // Parsed once; every logical CPU (BSP + APs) asks during its power init.
    static CAP: spin::Once<bool> = spin::Once::new();
    *CAP.call_once(|| {
        let cmdline = super::cmdline();
        cmdline.contains("noturbo") || cmdline.contains("turbo=off")
    })
}

// MWAIT idle hints (EAX). Bits [7:4] select the C-state, [3:0] the sub-state.
// 0x00 = C1, 0x01 = C1E. We deliberately go no deeper than C1E: C3+ can gate
// the LAPIC timer on parts without ARAT, and this kernel wakes idle CPUs from
// that timer, so a deeper state risks an over-long sleep / missed tick.
const MWAIT_HINT_C1: usize = 0x00;
const MWAIT_HINT_C1E: usize = 0x01;

/// Which P-state interface was enabled, for the boot log.
#[derive(Clone, Copy)]
enum PStateMech {
    IntelHwp,
    AmdCppc,
}

impl PStateMech {
    fn label(self) -> &'static str {
        match self {
            PStateMech::IntelHwp => "Intel HWP",
            PStateMech::AmdCppc => "AMD CPPC",
        }
    }
}

// ── Idle state shared with `wait_for_interrupt` ─────────────────────────────
/// Whether to idle via MONITOR/MWAIT (`true`) or fall back to `hlt` (`false`).
static IDLE_USE_MWAIT: AtomicBool = AtomicBool::new(false);
/// The MWAIT hint (C-state) chosen at init.
static IDLE_MWAIT_HINT: AtomicUsize = AtomicUsize::new(MWAIT_HINT_C1);

/// A cacheline that idle CPUs arm their MONITOR on. It is **never written**: we
/// rely solely on interrupts to break MWAIT, so this just gives MONITOR a valid
/// write-back address to watch. Aligned and padded to its own line so unrelated
/// stores never spuriously trip the monitor. Many CPUs may monitor it at once;
/// with no writes there are no store-wakeups, only the interrupt-wakeups we want.
#[repr(align(64))]
struct MonitorLine(AtomicU64);
static IDLE_MONITOR: MonitorLine = MonitorLine(AtomicU64::new(0));

/// Latches after the first CPU logs the power-management summary, so the APs
/// (which all run identical hardware) don't repeat it N times.
static SUMMARY_LOGGED: AtomicBool = AtomicBool::new(false);

// ── CPUID helpers ───────────────────────────────────────────────────────────
/// `true` when running under a hypervisor (CPUID.01H:ECX[31]). Reliably set by
/// QEMU/KVM and clear on bare metal, so it cleanly scopes this module to the
/// physical-hardware case it is meant for.
fn hypervisor_present() -> bool {
    __cpuid(1).ecx & (1 << 31) != 0
}

/// `true` on a "GenuineIntel" part.
fn is_intel() -> bool {
    let r = __cpuid(0);
    // "Genu" / "ineI" / "ntel" packed little-endian into EBX / EDX / ECX.
    r.ebx == 0x756e_6547 && r.edx == 0x4965_6e69 && r.ecx == 0x6c65_746e
}

/// `true` on an "AuthenticAMD" part.
fn is_amd() -> bool {
    let r = __cpuid(0);
    // "Auth" / "enti" / "cAMD" packed little-endian into EBX / EDX / ECX.
    r.ebx == 0x6874_7541 && r.edx == 0x6974_6e65 && r.ecx == 0x444d_4163
}

/// MSR: IA32_THERM_STATUS — per-core digital thermal sensor (DTS).
const IA32_THERM_STATUS: u32 = 0x19C;
/// MSR: IA32_TEMPERATURE_TARGET — TjMax (throttle temperature) in bits [23:16].
const MSR_TEMPERATURE_TARGET: u32 = 0x1A2;

/// This CPU's temperature in milli-degrees Celsius, or `None` when the hardware
/// doesn't expose it. Dispatches to the Intel (DTS via MSR) or AMD (SMN via the
/// Data Fabric) sensor. Skipped under a hypervisor: a VM may advertise the
/// sensor without implementing the backing MSR (which would #GP → panic), and
/// it never reports a real temperature anyway.
pub(crate) fn cpu_temperature_mc() -> Option<i32> {
    if hypervisor_present() {
        return None;
    }
    if is_intel() {
        intel_temperature_mc()
    } else if is_amd() {
        amd_temperature_mc()
    } else {
        None
    }
}

/// Intel digital thermal sensor (same source as the `coretemp` driver). The
/// sensor reports degrees *below* TjMax; the absolute temperature is
/// `TjMax - readout`. Gated on CPUID.06H:EAX[0] so the MSR read never #GPs.
fn intel_temperature_mc() -> Option<i32> {
    unsafe {
        if __cpuid(6).eax & 1 == 0 {
            return None; // no Digital Thermal Sensor
        }
        let status = Msr::new(IA32_THERM_STATUS).read();
        if status & (1 << 31) == 0 {
            return None; // reading not valid
        }
        let below_tjmax = ((status >> 16) & 0x7f) as i32;
        Some((intel_tjmax_c() - below_tjmax) * 1000)
    }
}

/// TjMax (throttle/junction temperature) in whole °C for an Intel part, falling
/// back to a sane 100 °C when `MSR_TEMPERATURE_TARGET` reads back zero.
///
/// SAFETY: reads `MSR_TEMPERATURE_TARGET`; only valid on a part with the DTS
/// (CPUID.06H:EAX[0]) — gated by the callers.
unsafe fn intel_tjmax_c() -> i32 {
    let t = ((Msr::new(MSR_TEMPERATURE_TARGET).read() >> 16) & 0xff) as i32;
    if t > 0 {
        t
    } else {
        100
    }
}

// ── AMD temperature (k10temp-style SMN read) ────────────────────────────────
/// SMN address of the reported-temperature control register on Family 17h+.
const ZEN_REPORTED_TEMP_CTRL: u32 = 0x0005_9800;
/// `CurTmp` field starts at bit 21 (each step is 0.125 °C = 125 m°C).
const ZEN_CUR_TEMP_SHIFT: u32 = 21;
/// When set, `CurTmp` uses the extended range and is offset by -49 °C.
const ZEN_CUR_TEMP_RANGE_SEL: u32 = 1 << 19;

/// Read a 32-bit PCI config dword via the legacy 0xCF8/0xCFC mechanism, with
/// interrupts masked so the address→data pair can't be torn by a local IRQ that
/// touches PCI config space.
unsafe fn pci_cfg_read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    use x86_64::instructions::interrupts;
    use zcore_drivers::io::{Io, Pmio};
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((off as u32) & 0xFC);
    interrupts::without_interrupts(|| {
        Pmio::<u32>::new(0xCF8).write(addr);
        Pmio::<u32>::new(0xCFC).read()
    })
}

/// Write `addr` to the Data Fabric SMN index register then read the data
/// register (D18F0, offsets 0x60/0x64) — the standard AMD SMN indirect access.
unsafe fn amd_smn_read(smn_addr: u32) -> u32 {
    use x86_64::instructions::interrupts;
    use zcore_drivers::io::{Io, Pmio};
    let index = 0x8000_0000u32 | (0x18 << 11) | (0 << 8) | 0x60;
    let data = 0x8000_0000u32 | (0x18 << 11) | (0 << 8) | 0x64;
    interrupts::without_interrupts(|| {
        // index register (0x60)
        Pmio::<u32>::new(0xCF8).write(index);
        Pmio::<u32>::new(0xCFC).write(smn_addr);
        // data register (0x64)
        Pmio::<u32>::new(0xCF8).write(data);
        Pmio::<u32>::new(0xCFC).read()
    })
}

/// Serializes the legacy 0xCF8/0xCFC PCI-config address→data pair used for the
/// AMD SMN temperature read. A single shared register pair means two CPUs
/// reading the package `Tctl` concurrently (e.g. the per-core thermal governor
/// firing on several cores at once, or a `/sys/class/thermal` read racing it)
/// could interleave one core's index write with another's data read and return
/// garbage. `without_interrupts` inside the read only stops *local* reentrancy,
/// so a cross-core lock is required.
static AMD_SMN_LOCK: Mutex<()> = Mutex::new(());

/// Inner AMD `Tctl` read, in milli-°C. Caller must hold [`AMD_SMN_LOCK`].
unsafe fn amd_temperature_raw() -> Option<i32> {
    let eax = __cpuid(1).eax;
    let base_family = (eax >> 8) & 0xf;
    let ext_family = (eax >> 20) & 0xff;
    let family = if base_family == 0xf {
        base_family + ext_family
    } else {
        base_family
    };
    if family < 0x17 {
        return None; // pre-Zen uses a different (older) path; not supported
    }
    // Confirm the Data Fabric function 0 really is an AMD device before
    // trusting the SMN window (vendor id 0x1022 in the low 16 bits).
    if pci_cfg_read32(0, 0x18, 0, 0x00) & 0xFFFF != 0x1022 {
        return None;
    }
    let regval = amd_smn_read(ZEN_REPORTED_TEMP_CTRL);
    let mut temp = ((regval >> ZEN_CUR_TEMP_SHIFT) as i32) * 125;
    if regval & ZEN_CUR_TEMP_RANGE_SEL != 0 {
        temp -= 49_000;
    }
    Some(temp)
}

/// AMD core temperature (`Tctl`), Family 17h (Zen) and later, read from the SMU
/// over the Data Fabric SMN — the mechanism the Linux `k10temp` driver uses.
/// Returns `Tctl` in milli-degrees C (the per-model `Tdie` offset some Ryzen /
/// Threadripper parts apply is not subtracted). Blocks on [`AMD_SMN_LOCK`].
fn amd_temperature_mc() -> Option<i32> {
    let _guard = AMD_SMN_LOCK.lock();
    unsafe { amd_temperature_raw() }
}

/// Like [`amd_temperature_mc`] but never blocks: returns `None` if the SMN lock
/// is contended. Used by the governor, which runs in timer-IRQ context where it
/// must not spin waiting on a `/sys` reader (and can simply skip this sample).
fn amd_temperature_mc_try() -> Option<i32> {
    let _guard = AMD_SMN_LOCK.try_lock()?;
    unsafe { amd_temperature_raw() }
}

/// `true` if AMD Collaborative Processor Performance Control is present
/// (CPUID Fn8000_0008_EBX[27]). Guards access to the MSR_AMD_CPPC_* registers.
fn amd_has_cppc() -> bool {
    if __cpuid(0x8000_0000).eax < 0x8000_0008 {
        return false;
    }
    __cpuid(0x8000_0008).ebx & (1 << 27) != 0
}

// ── P-state programming (per logical CPU) ───────────────────────────────────
/// Enable Intel HWP on this CPU and request hardware-autonomous scaling across
/// the full [lowest, highest] range. Returns `(lowest, highest)` for logging.
///
/// SAFETY: writes IA32_PM_ENABLE / IA32_HWP_REQUEST, valid only when HWP is
/// supported (checked by the caller).
unsafe fn enable_hwp(has_epp: bool) -> (u8, u8, bool) {
    // Turn HWP on (idempotent / sticky); IA32_HWP_REQUEST is only meaningful
    // once this bit is set.
    Msr::new(IA32_PM_ENABLE).write(HWP_ENABLE);

    let caps = Msr::new(IA32_HWP_CAPABILITIES).read();
    let highest = (caps & 0xff) as u8; // [7:0]   Highest_Performance (turbo)
    let guaranteed = ((caps >> 8) & 0xff) as u8; // [15:8]  Guaranteed_Performance (base)
    let lowest = ((caps >> 24) & 0xff) as u8; // [31:24] Lowest_Performance

    // Cap the ceiling at the base clock to disable turbo, unless `guaranteed` is
    // unreported (0) or nonsensical, in which case keep the full range.
    let cap = cap_at_base_clock() && guaranteed >= lowest && guaranteed > 0;
    let max = if cap { guaranteed } else { highest };

    // Minimum = lowest → an idle core may drop to its lowest P-state (coolest).
    // Maximum = max    → the ceiling (base clock when turbo is capped).
    // Desired = 0      → hardware chooses the operating point autonomously.
    // EPP only exists when CPUID.06H:EAX[10] is set; otherwise bits [31:24] are
    // reserved-zero and IA32_ENERGY_PERF_BIAS provides the bias instead.
    let epp = if has_epp { EPP_PREF } else { 0 };
    let request = (lowest as u64) | ((max as u64) << 8) | (0u64 << 16) | (epp << 24);
    Msr::new(IA32_HWP_REQUEST).write(request);

    (lowest, max, cap)
}

/// Enable AMD CPPC on this CPU and request hardware-autonomous scaling across
/// the full [lowest, highest] range. Returns `(lowest, highest)` for logging.
///
/// SAFETY: writes the MSR_AMD_CPPC_* registers, valid only when CPPC is present
/// (checked by the caller). Note the field order differs from Intel HWP.
unsafe fn enable_amd_cppc() -> (u8, u8, bool) {
    Msr::new(MSR_AMD_CPPC_ENABLE).write(1);

    let cap1 = Msr::new(MSR_AMD_CPPC_CAP1).read();
    let highest = ((cap1 >> 24) & 0xff) as u8; // [31:24] Highest_Performance (boost)
    let nominal = ((cap1 >> 16) & 0xff) as u8; // [23:16] Nominal_Performance (base)
    let lowest = (cap1 & 0xff) as u8; // [7:0]    Lowest_Performance

    // Cap the ceiling at nominal (base) to disable Precision Boost, unless it is
    // unreported (0) or nonsensical.
    let cap = cap_at_base_clock() && nominal >= lowest && nominal > 0;
    let max = if cap { nominal } else { highest };

    // REQUEST: Max[7:0]=max, Min[15:8]=lowest, Desired[23:16]=0 (autonomous),
    // EPP[31:24]. Out-of-range fields are clamped by hardware to [lowest,highest].
    let request = (max as u64) | ((lowest as u64) << 8) | (0u64 << 16) | (EPP_PREF << 24);
    Msr::new(MSR_AMD_CPPC_REQUEST).write(request);

    (lowest, max, cap)
}

/// Set the legacy Intel Energy-Performance Bias hint, preserving reserved bits.
///
/// SAFETY: writes IA32_ENERGY_PERF_BIAS, valid only when CPUID.06H:ECX[3] is
/// set (checked by the caller).
unsafe fn set_energy_perf_bias() {
    let mut msr = Msr::new(IA32_ENERGY_PERF_BIAS);
    let value = (msr.read() & !0xf) | EPB_PREF;
    msr.write(value);
}

// ── Public entry points ─────────────────────────────────────────────────────
/// Configure CPU power management for the calling logical CPU. Safe to call on
/// the BSP and on every AP — the HWP/CPPC/EPB MSRs are per-logical-CPU, so each
/// core must run it; the MWAIT-idle decision is a global latched once. No-ops on
/// a hypervisor or where CPUID reports the features absent.
pub(super) fn init() {
    if hypervisor_present() {
        log_summary_once(|| {
            info!("power: under hypervisor — leaving CPU power management to the host");
        });
        return;
    }

    let intel = is_intel();
    let amd = !intel && is_amd();
    let leaf1 = __cpuid(1);
    let leaf6 = __cpuid(6);

    // --- Hardware-autonomous P-states ---
    // Intel exposes HWP; AMD exposes the equivalent via CPPC. Both, once enabled,
    // scale the core autonomously across [lowest, highest], so an idle core drops
    // to its lowest voltage/frequency. EPB is an Intel-only legacy hint for parts
    // predating (or lacking the EPP field of) HWP.
    let has_hwp = intel && (leaf6.eax & (1 << 7)) != 0;
    let has_hwp_epp = (leaf6.eax & (1 << 10)) != 0;
    let has_epb = intel && (leaf6.ecx & (1 << 3)) != 0;

    let pstate = if has_hwp {
        Some((PStateMech::IntelHwp, unsafe { enable_hwp(has_hwp_epp) }))
    } else if amd && amd_has_cppc() {
        Some((PStateMech::AmdCppc, unsafe { enable_amd_cppc() }))
    } else {
        None
    };
    if has_epb {
        unsafe { set_energy_perf_bias() };
    }

    // Record this CPU's P-state bounds so the adaptive thermal governor can
    // walk its ceiling down/up at runtime (see `thermal_governor_tick`).
    governor_init_cpu(pstate, has_hwp_epp);

    // --- Idle C-state via MONITOR/MWAIT (cross-vendor) ---
    let has_monitor = (leaf1.ecx & (1 << 3)) != 0;
    let leaf5 = __cpuid(5);
    let mwait_pm_ext = (leaf5.ecx & (1 << 0)) != 0; // MWAIT power-mgmt hints usable
    let mwait_on = has_monitor && mwait_pm_ext;
    if mwait_on {
        // C1E lives as sub-state ≥1 of C1: only use it when CPUID.05H reports
        // more than one C1 sub-state (EDX[7:4]); otherwise stick to plain C1.
        let c1_substates = (leaf5.edx >> 4) & 0xf;
        let hint = if c1_substates >= 2 {
            MWAIT_HINT_C1E
        } else {
            MWAIT_HINT_C1
        };
        IDLE_MWAIT_HINT.store(hint, Ordering::Relaxed);
        // MWAIT idle is left DISABLED: on real hardware the C-state it enters has
        // been observed to stop the LAPIC, and with the kernel's tickless idle a
        // timer-only wake is then lost — the machine hangs at boot right after
        // the splash logo, before the shell prompt (QEMU never hit this because
        // it idles via `hlt`). All idle paths therefore park in C1 via `hlt`,
        // which never stops the LAPIC. Re-enabling MWAIT/C1E needs per-machine
        // validation that the LAPIC keeps ticking.
        //
        // IDLE_USE_MWAIT.store(true, Ordering::Relaxed);  // intentionally off
    }

    log_summary_once(|| {
        match pstate {
            Some((mech, (lo, hi, capped))) => info!(
                "power: {} enabled — autonomous P-state {}..={}{}, EPP={:#04x}{}",
                mech.label(),
                lo,
                hi,
                if capped {
                    " (noturbo: turbo/boost disabled)"
                } else {
                    " (turbo/boost allowed)"
                },
                EPP_PREF,
                if has_epb { " +EPB" } else { "" },
            ),
            None => info!(
                "power: no OS P-state control ({}); left to firmware{}",
                if intel {
                    "Intel without HWP"
                } else if amd {
                    "AMD without CPPC"
                } else {
                    "unknown CPU vendor"
                },
                if has_epb { ", EPB set" } else { "" },
            ),
        }
        // Report the idle path that is *actually* used, not merely what the CPU
        // could do. MWAIT/C1E is currently force-disabled (`IDLE_USE_MWAIT` is
        // never set) because the deeper C-state stops the LAPIC on some real
        // hardware and the tickless-idle wake is then lost (boot hangs right
        // after the splash logo). Logging "MONITOR/MWAIT" while really halting
        // via `hlt` would send any boot-hang investigation down the wrong path,
        // so key the message on the live flag.
        if IDLE_USE_MWAIT.load(Ordering::Relaxed) {
            let hint = IDLE_MWAIT_HINT.load(Ordering::Relaxed);
            info!(
                "power: idle via MONITOR/MWAIT ({})",
                if hint == MWAIT_HINT_C1E { "C1E" } else { "C1" },
            );
        } else if mwait_on {
            info!("power: idle via HLT (C1) — MWAIT/C1E available but disabled (LAPIC-safe)");
        } else {
            info!("power: idle via HLT (C1)");
        }
    });
}

/// Idle the calling CPU until the next interrupt. Replaces a bare `sti; hlt`:
/// parks in C1E via MONITOR/MWAIT when available (cooler), else halts. Restores
/// the caller's interrupt-enable state, matching `enable_and_hlt` semantics.
pub(super) fn cpu_idle() {
    use x86_64::instructions::interrupts;

    let was_enabled = interrupts::are_enabled();
    let idle_start = crate::hal_fn::timer::timer_now();
    crate::kstats::set_cpu_idle(true);

    if IDLE_USE_MWAIT.load(Ordering::Relaxed) {
        let hint = IDLE_MWAIT_HINT.load(Ordering::Relaxed);
        // Arm the monitor with interrupts masked so nothing slips between the
        // MONITOR and the MWAIT, then `sti; mwait`: the one-instruction shadow
        // after STI guarantees MWAIT is entered before any interrupt is taken,
        // and a pending/new interrupt then breaks MWAIT — so no wake is ever
        // lost (the same guarantee `enable_and_hlt`'s `sti; hlt` relies on).
        interrupts::disable();
        let monitor_addr = &IDLE_MONITOR as *const _ as usize;
        unsafe {
            core::arch::asm!(
                "monitor",
                in("rax") monitor_addr,
                in("rcx") 0,
                in("rdx") 0,
                options(nostack, preserves_flags),
            );
            core::arch::asm!(
                "sti; mwait",
                in("rax") hint,
                in("rcx") 0,
                options(nostack),
            );
        }
    } else {
        interrupts::enable_and_hlt();
    }

    if !was_enabled {
        interrupts::disable();
    }
    crate::kstats::set_cpu_idle(false);

    let idle_ns = crate::hal_fn::timer::timer_now()
        .checked_sub(idle_start)
        .unwrap_or_default()
        .as_nanos() as u64;
    crate::kstats::note_idle(idle_ns);
}

/// FFI entry used by the scheduler (`PreemptiveScheduler`) to idle the CPU. The
/// scheduler can't depend on `kernel-hal` (that would be circular), so it calls
/// this symbol — the same pattern as the `drivers_*` shims.
///
/// This deliberately parks in **C1 via a plain `sti; hlt`**, NOT the MONITOR/
/// MWAIT C1E path: on some real hardware the deeper C-state MWAIT can enter
/// stops the LAPIC, and with tickless idle a timer-only wake is then lost — the
/// machine hangs at boot right after the splash logo, before the shell prompt
/// ever appears (QEMU never hit this because it idles via `hlt`). C1 never stops
/// the LAPIC, so the tickless wake is always delivered. Idle time is still
/// accounted for `/proc/perf/kernel`.
#[no_mangle]
extern "C" fn hal_cpu_idle() {
    use x86_64::instructions::interrupts;

    let was_enabled = interrupts::are_enabled();
    let start = crate::hal_fn::timer::timer_now();
    crate::kstats::set_cpu_idle(true);
    // Seal the stack window above this frame for the duration of the halt: the
    // wake IRQ verifies it qword-for-qword before anything else runs. Catches
    // the recurring "ret pops 0 after idle" corruption at the moment of wake,
    // with an exact diff of what changed. See trap.rs `IdleSeal`.
    //
    // The anchor local is the seal's base address. A local is at or above this
    // frame's RSP by the SysV ABI, while the return-address pushes of the
    // `call`s below go below it — which is exactly the line between "constant
    // during the halt" and "legitimately rewritten between two wakes". Sealing
    // from the callee's RSP instead captured the outgoing-call slot and a
    // second wake IPI arriving mid-`call idle_stack_unseal` reported that
    // slot's legitimate re-push as corruption (see `idle_stack_seal`'s doc).
    let seal_anchor: u64 = 0;
    super::trap::idle_stack_seal(&seal_anchor as *const u64 as usize);
    interrupts::enable_and_hlt();
    super::trap::idle_stack_unseal();
    // Keep the anchor's stack slot alive (and thus stable) across the sealed
    // window — without this the compiler may reuse it for a spill, putting a
    // legitimate write inside the sealed region.
    core::hint::black_box(&seal_anchor);
    if !was_enabled {
        interrupts::disable();
    }
    crate::kstats::set_cpu_idle(false);
    let ns = crate::hal_fn::timer::timer_now()
        .checked_sub(start)
        .unwrap_or_default()
        .as_nanos() as u64;
    crate::kstats::note_idle(ns);
}

// ── Thermal-adaptive P-state governor ───────────────────────────────────────
//
// The init-time P-state policy is static: ceiling = base clock, floor = lowest.
// That keeps an idle core cool, but under sustained load the package can still
// climb. This governor closes the loop: each logical CPU samples its own
// temperature ~1 Hz from the timer tick and walks its HWP/CPPC *ceiling* down
// (below base, toward lowest) as it nears TjMax, then back up to base as it
// cools — proactive, gradual throttling that engages before the hardware's hard
// PROCHOT wall and smooths thermal cycling. It never touches C-states or the
// LAPIC (so it can't reintroduce the deep-idle boot hang), and only writes the
// per-CPU request MSR when the chosen ceiling actually changes; the steady-state
// cost is one cheap temperature read per second per core.

/// Master switch. With it off, the static init-time P-state policy stands.
const GOVERNOR_ENABLED: bool = true;
/// Minimum spacing between governor evaluations on a given logical CPU.
const GOV_INTERVAL_NS: u64 = 1_000_000_000; // 1 s
/// Logical CPUs the per-core governor tracks. Cores past this keep their
/// init-time ceiling (no adaptive throttling); the array index just bails.
const GOV_MAX_CPUS: usize = 256;
/// AMD `Tctl` throttle band, milli-°C (Zen exposes no fixed TjMax MSR).
const GOV_AMD_HOT_MC: i32 = 88_000;
const GOV_AMD_COOL_MC: i32 = 78_000;
/// Intel band as a °C offset below TjMax.
const GOV_INTEL_HOT_BELOW_TJMAX: i32 = 12;
const GOV_INTEL_COOL_BELOW_TJMAX: i32 = 22;

/// P-state mechanism in use: 0 = none, 1 = Intel HWP, 2 = AMD CPPC.
static GOV_MECH: AtomicU8 = AtomicU8::new(0);
/// Whether the HWP request carries an EPP field (mirrors `enable_hwp`).
static GOV_HAS_EPP: AtomicBool = AtomicBool::new(false);

// Per-logical-CPU state. Separate atomic arrays avoid a non-`Copy` struct-array
// initializer in a `static`.
static GOV_VALID: [AtomicBool; GOV_MAX_CPUS] = [const { AtomicBool::new(false) }; GOV_MAX_CPUS];
static GOV_LOWEST: [AtomicU8; GOV_MAX_CPUS] = [const { AtomicU8::new(0) }; GOV_MAX_CPUS];
/// The normal (init-time) ceiling — base clock when turbo is capped.
static GOV_CEIL_MAX: [AtomicU8; GOV_MAX_CPUS] = [const { AtomicU8::new(0) }; GOV_MAX_CPUS];
/// The currently programmed ceiling (GOV_LOWEST ..= GOV_CEIL_MAX).
static GOV_CEILING: [AtomicU8; GOV_MAX_CPUS] = [const { AtomicU8::new(0) }; GOV_MAX_CPUS];
static GOV_LAST_NS: [AtomicU64; GOV_MAX_CPUS] = [const { AtomicU64::new(0) }; GOV_MAX_CPUS];
static GOV_THROTTLED: [AtomicBool; GOV_MAX_CPUS] = [const { AtomicBool::new(false) }; GOV_MAX_CPUS];

/// Record the calling CPU's P-state bounds so the governor can scale its
/// ceiling. Called once per logical CPU from `init`, after HWP/CPPC is set.
fn governor_init_cpu(pstate: Option<(PStateMech, (u8, u8, bool))>, has_epp: bool) {
    let (mech, (lowest, max, _capped)) = match pstate {
        Some(p) => p,
        None => return, // no OS P-state control → governor inert
    };
    let cpu = lock::current_cpu_id() as usize;
    if cpu < GOV_MAX_CPUS {
        GOV_LOWEST[cpu].store(lowest, Ordering::Relaxed);
        GOV_CEIL_MAX[cpu].store(max, Ordering::Relaxed);
        GOV_CEILING[cpu].store(max, Ordering::Relaxed);
        GOV_LAST_NS[cpu].store(0, Ordering::Relaxed);
        GOV_THROTTLED[cpu].store(false, Ordering::Relaxed);
        GOV_VALID[cpu].store(true, Ordering::Release);
    }
    GOV_HAS_EPP.store(has_epp, Ordering::Relaxed);
    GOV_MECH.store(
        match mech {
            PStateMech::IntelHwp => 1,
            PStateMech::AmdCppc => 2,
        },
        Ordering::Release,
    );
}

/// `(cool_mc, hot_mc)` governor hysteresis band for the running vendor.
fn governor_band_mc() -> (i32, i32) {
    if is_intel() {
        let tjmax = unsafe { intel_tjmax_c() };
        let cool = (tjmax - GOV_INTEL_COOL_BELOW_TJMAX).max(40) * 1000;
        let hot = (tjmax - GOV_INTEL_HOT_BELOW_TJMAX).max(50) * 1000;
        (cool, hot)
    } else {
        (GOV_AMD_COOL_MC, GOV_AMD_HOT_MC)
    }
}

/// Non-blocking temperature read for the governor (timer-IRQ context).
fn governor_temperature_mc() -> Option<i32> {
    if is_intel() {
        intel_temperature_mc()
    } else if is_amd() {
        amd_temperature_mc_try()
    } else {
        None
    }
}

/// Program a new P-state ceiling (max field) on the calling CPU, keeping the
/// floor at `lowest` and the power-save EPP — same request layout as
/// `enable_hwp` / `enable_amd_cppc`, just with an adjusted maximum.
///
/// SAFETY: writes IA32_HWP_REQUEST / MSR_AMD_CPPC_REQUEST; valid only when the
/// matching mechanism is active (gated via `GOV_MECH` by the caller).
unsafe fn governor_program_ceiling(mech: u8, lowest: u8, max: u8) {
    match mech {
        1 => {
            let epp = if GOV_HAS_EPP.load(Ordering::Relaxed) {
                EPP_PREF
            } else {
                0
            };
            let request = (lowest as u64) | ((max as u64) << 8) | (0u64 << 16) | (epp << 24);
            Msr::new(IA32_HWP_REQUEST).write(request);
        }
        2 => {
            let request = (max as u64) | ((lowest as u64) << 8) | (0u64 << 16) | (EPP_PREF << 24);
            Msr::new(MSR_AMD_CPPC_REQUEST).write(request);
        }
        _ => {}
    }
}

/// One adaptive-governor step for the calling CPU. Cheap and self-rate-limited
/// to `GOV_INTERVAL_NS`, so it is safe to call from every timer tick. No-ops
/// under a hypervisor or where no OS P-state control was enabled.
pub(crate) fn thermal_governor_tick() {
    if !GOVERNOR_ENABLED {
        return;
    }
    let mech = GOV_MECH.load(Ordering::Acquire);
    if mech == 0 {
        return; // no P-state control (or pre-init / hypervisor)
    }
    let cpu = lock::current_cpu_id() as usize;
    if cpu >= GOV_MAX_CPUS || !GOV_VALID[cpu].load(Ordering::Acquire) {
        return;
    }

    // Self-rate-limit. This slot is only ever touched by its own CPU, and a
    // timer tick can't re-enter itself, so a plain load/store is race-free.
    let now = crate::hal_fn::timer::timer_now().as_nanos() as u64;
    if now.wrapping_sub(GOV_LAST_NS[cpu].load(Ordering::Relaxed)) < GOV_INTERVAL_NS {
        return;
    }
    GOV_LAST_NS[cpu].store(now, Ordering::Relaxed);

    let lowest = GOV_LOWEST[cpu].load(Ordering::Relaxed);
    let ceil_max = GOV_CEIL_MAX[cpu].load(Ordering::Relaxed);
    if ceil_max <= lowest {
        return; // no room to scale
    }
    let temp = match governor_temperature_mc() {
        Some(t) => t,
        None => return,
    };
    let (cool, hot) = governor_band_mc();

    let ceiling = GOV_CEILING[cpu].load(Ordering::Relaxed);
    // One step ≈ 1/8 of the dynamic range → a full swing takes ~8 s: gentle
    // enough to avoid oscillation, quick enough to react before PROCHOT.
    let step = core::cmp::max(1, (ceil_max - lowest) / 8);
    let new_ceiling = if temp >= hot {
        ceiling.saturating_sub(step).max(lowest)
    } else if temp <= cool {
        core::cmp::min(ceil_max, ceiling.saturating_add(step))
    } else {
        return; // inside the hysteresis band: hold
    };
    if new_ceiling == ceiling {
        return;
    }

    unsafe { governor_program_ceiling(mech, lowest, new_ceiling) };
    GOV_CEILING[cpu].store(new_ceiling, Ordering::Relaxed);

    let now_throttled = new_ceiling < ceil_max;
    let was_throttled = GOV_THROTTLED[cpu].swap(now_throttled, Ordering::Relaxed);
    if now_throttled != was_throttled {
        info!(
            "power: cpu{} thermal governor {} — ceiling {}/{} (~{}.{} C)",
            cpu,
            if now_throttled {
                "throttling"
            } else {
                "released"
            },
            new_ceiling,
            ceil_max,
            temp / 1000,
            (temp % 1000).abs() / 100,
        );
    }
}

/// Adaptive-governor summary for `/proc/perf`: `(throttled core count, cpu0's
/// current ceiling, cpu0's base ceiling)`, or `None` when the governor is inert.
pub(crate) fn governor_summary() -> Option<(u32, u8, u8)> {
    if GOV_MECH.load(Ordering::Acquire) == 0 || !GOV_VALID[0].load(Ordering::Acquire) {
        return None;
    }
    let throttled = (0..GOV_MAX_CPUS)
        .filter(|&c| {
            GOV_VALID[c].load(Ordering::Relaxed) && GOV_THROTTLED[c].load(Ordering::Relaxed)
        })
        .count() as u32;
    Some((
        throttled,
        GOV_CEILING[0].load(Ordering::Relaxed),
        GOV_CEIL_MAX[0].load(Ordering::Relaxed),
    ))
}

/// Run `f` only on the first CPU to reach it; the APs run identical hardware, so
/// the summary need only be logged once.
fn log_summary_once(f: impl FnOnce()) {
    if SUMMARY_LOGGED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        f();
    }
}

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use spin::Once;
use x86_64::instructions::port::Port;

/// Global monotonic floor in nanoseconds. Unsynchronized per-CPU TSCs can read
/// backwards across cores; smoltcp's TCP timers (and every sleep/timeout in the
/// kernel) require non-decreasing time, so clamp each reading to the highest
/// value observed on any CPU.
///
/// Only consulted on the slow path: with an invariant TSC (the norm on every
/// post-2008 x86 and on QEMU/KVM) the package synchronizes the counters at
/// reset, so the raw reading is already cross-CPU monotonic and the floor RMW
/// — a single cacheline every CPU would otherwise bounce on EVERY clock read
/// (syscall entry/exit, every tick, hunter's time source) — is skipped. A
/// per-tick watchdog (`mono_floor_tick`) keeps maintaining the floor at 250 Hz
/// and demotes to the clamped slow path if real skew is ever observed.
static MONO_NS: AtomicU64 = AtomicU64::new(0);

/// `true` once boot detected an invariant TSC (CPUID.80000007H:EDX[8]) — the
/// fast, RMW-free `timer_now` path. Cleared forever by `mono_floor_tick` if a
/// CPU observes the floor ahead of its own reading beyond the tolerance.
static TSC_INVARIANT: AtomicBool = AtomicBool::new(false);

/// Fixed-point multiplier: ns = (tsc * TSC_NS_MULT) >> 32, i.e.
/// `(1000 << 32) / freq_mhz`. Replaces the per-read 64-bit division and, via
/// the 128-bit intermediate, fixes the `cycles * 1000` overflow that wrapped
/// the clock after ~71 days of uptime at 3 GHz. 0 = not yet initialized.
static TSC_NS_MULT: AtomicU64 = AtomicU64::new(0);

/// Skew tolerance for the invariant-TSC watchdog. Same-package TSCs agree to
/// within nanoseconds; the floor a CPU compares against can be up to one tick
/// (4 ms) stale, so anything beyond a generous 1 ms means genuinely unsynced
/// counters (multi-socket, buggy firmware) and demotes to the clamped path.
const TSC_SKEW_TOLERANCE_NS: u64 = 1_000_000;

#[cold]
fn tsc_ns_mult_init() -> u64 {
    // cpu_frequency() is Once-cached and never zero (falls back to 2000 MHz).
    let mult = (1000u64 << 32) / super::cpu::cpu_frequency() as u64;
    TSC_NS_MULT.store(mult, Ordering::Relaxed);
    // Invariant TSC: CPUID leaf 0x8000_0007, EDX bit 8. On such parts the TSC
    // runs at a constant rate and, on a single package, is reset-synchronized
    // across cores, so raw readings are already monotonic system-wide.
    let invariant = raw_cpuid::CpuId::new()
        .get_extended_function_info()
        .map(|info| info.has_invariant_tsc())
        .unwrap_or(false);
    TSC_INVARIANT.store(invariant, Ordering::Relaxed);
    mult
}

#[inline]
fn tsc_to_ns(cycle: u64) -> u64 {
    let mut mult = TSC_NS_MULT.load(Ordering::Relaxed);
    if mult == 0 {
        mult = tsc_ns_mult_init();
    }
    ((cycle as u128 * mult as u128) >> 32) as u64
}

pub fn timer_now() -> Duration {
    let cycle = unsafe { core::arch::x86_64::_rdtsc() };
    let ns = tsc_to_ns(cycle);
    if TSC_INVARIANT.load(Ordering::Relaxed) {
        // Fast path: no shared-cacheline RMW. The floor is still advanced at
        // tick rate by `mono_floor_tick`, so a later demotion to the slow path
        // stays (almost) seamless.
        return Duration::from_nanos(ns);
    }
    // `fetch_max` returns the previous value; the effective clock is the larger
    // of the previous floor and this reading, guaranteeing it never goes back.
    let prev = MONO_NS.fetch_max(ns, Ordering::Relaxed);
    Duration::from_nanos(prev.max(ns))
}

/// Per-tick watchdog for the invariant-TSC fast path, called from every CPU's
/// periodic tick (250 Hz × N CPUs — negligible RMW traffic). Keeps the floor
/// fresh and demotes to the clamped slow path if this CPU's TSC reading is
/// ever behind the floor by more than the tolerance, which would mean the
/// "invariant" TSCs are not actually synchronized on this machine.
pub fn mono_floor_tick(now_ns: u64) {
    if !TSC_INVARIANT.load(Ordering::Relaxed) {
        return; // slow path already maintains the floor on every read
    }
    let prev = MONO_NS.fetch_max(now_ns, Ordering::Relaxed);
    if prev > now_ns + TSC_SKEW_TOLERANCE_NS {
        TSC_INVARIANT.store(false, Ordering::Relaxed);
        warn!(
            "TSC skew detected ({} ns behind the cross-CPU floor); \
             falling back to the clamped monotonic clock",
            prev - now_ns
        );
        // The clamped path exists inside this kernel only. Userspace reading
        // the TSC through the vDSO has no way to participate in the floor, so
        // once the counters are known to disagree across CPUs the vDSO must
        // stop answering and send everyone back to the syscall — otherwise a
        // thread that migrates reads time going backwards.
        crate::timer::notify_clock_changed();
    }
}

/// The TSC→ns multiplier, but only when the TSC is fit to be read directly by
/// userspace. `None` means the vDSO must stay disabled.
///
/// Two conditions, and both are load-bearing. The multiplier must exist, which
/// it does not until the first `timer_now` calibrates it. And the TSC must be
/// invariant: constant-rate, so one multiplier is valid for all time, and
/// reset-synchronized across cores, so a thread that migrates mid-read cannot
/// see the clock go backwards. When it is, `timer_now` returns exactly
/// `tsc_to_ns(rdtsc())` with no floor applied — the same arithmetic on the same
/// inputs the vDSO performs, so the two clocks cannot disagree.
pub fn vdso_tsc_mult() -> Option<u64> {
    if !TSC_INVARIANT.load(Ordering::Relaxed) && !FORCE_TSC_INVARIANT.load(Ordering::Relaxed) {
        return None;
    }
    match TSC_NS_MULT.load(Ordering::Relaxed) {
        0 => None,
        mult => Some(mult),
    }
}

/// Set by `VDSOFORCE=1` on the kernel command line: treat the TSC as usable by
/// userspace even though CPUID does not say it is invariant.
///
/// This exists because QEMU cannot advertise an invariant TSC under TCG — the
/// feature word is not in its TCG-supported set, so `+invtsc` is dropped — and
/// TCG is the only substrate on which Eclipse and Linux can be compared on
/// equal terms. Without it the vDSO would be unmeasurable. See `zCore/main.rs`
/// for why it is sound there and unsound on real hardware.
///
/// Deliberately does NOT touch `TSC_INVARIANT`: the kernel's own monotonic
/// floor keeps working exactly as it did, so forcing this affects what
/// userspace is allowed to do and nothing else.
static FORCE_TSC_INVARIANT: AtomicBool = AtomicBool::new(false);

/// Enable the `VDSOFORCE=1` override. See [`FORCE_TSC_INVARIANT`].
pub fn set_force_tsc_invariant(force: bool) {
    FORCE_TSC_INVARIANT.store(force, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Tickless-idle LAPIC timer re-arming
// ---------------------------------------------------------------------------
// The LAPIC timer counts raw CPU cycles: boot programs `TimerDivide::Div256`,
// which on this hardware behaves as divide-by-1 (see `drivers.rs`). So the
// initial-count register is just `cycles`. We modulate that count to stretch
// the periodic tick when a CPU goes idle, then restore it on resume.

use super::super::timer::TICKS_PER_SEC;
use zcore_drivers::irq::x86::Apic;

/// LAPIC timer initial count for the normal full-rate scheduler tick (4 ms at
/// 250 Hz). Mirrors the value programmed in `drivers.rs` at boot.
pub fn fast_tick_count() -> u32 {
    (super::cpu::cpu_frequency() as u64 * 1_000_000 / TICKS_PER_SEC) as u32
}

/// Period of the full-rate scheduler tick, in nanoseconds (4 ms at 250 Hz).
/// The upper bound on how far ahead the deadline timer is ever programmed:
/// preemption and the per-tick housekeeping must keep running regardless of
/// what the timer heap wants.
pub const fn fast_tick_ns() -> u64 {
    1_000_000_000 / TICKS_PER_SEC
}

/// Convert a now-relative nanosecond span to LAPIC timer cycles. `cpu_frequency`
/// is in MHz (= cycles per microsecond). Clamped to a non-zero `u32`: a count of
/// 0 stops the timer, and counts above `u32::MAX` are not representable.
pub fn ns_to_tick_count(ns: u64) -> u32 {
    let cycles = (super::cpu::cpu_frequency() as u64).saturating_mul(ns) / 1000;
    cycles.clamp(1, u32::MAX as u64) as u32
}

/// Reprogram this CPU's LAPIC timer initial count (the period, in periodic
/// mode). Safe from any CPU: the LAPIC registers are per-CPU hardware reached
/// through the local MMIO window / MSRs.
pub fn set_tick_count(count: u32) {
    if Apic::local_apic_ready() {
        Apic::local_apic().set_timer_initial(count);
    }
}

/// Program *this* CPU's LAPIC timer for the periodic scheduler tick.
///
/// The BSP does this inline during primary init (`drivers.rs`). Each AP must
/// repeat it: the LAPIC timer's mode / divide / initial-count registers are
/// per-CPU hardware and are NOT inherited from the BSP — only the shared cached
/// config (vector) is. An AP that skips this is left with an initial count of 0,
/// i.e. a *stopped* timer, so it never takes the 250 Hz tick: no preemption, no
/// idle accounting, and the whole system's `naive_timer` heap ends up serviced
/// by the BSP alone — which shows up as a lopsided per-CPU busy split and an
/// inflated `/proc/perf/kernel` busy%. Leaves the timer masked; the unmask
/// happens later via `apic_timer_enable()` (same ordering as the BSP).
pub fn program_periodic_tick() {
    use x2apic::lapic::{TimerDivide, TimerMode};
    if Apic::local_apic_ready() {
        let lapic = Apic::local_apic();
        lapic.set_timer_mode(TimerMode::Periodic);
        lapic.set_timer_divide(TimerDivide::Div256); // actually Div1 (crate naming quirk)
        lapic.set_timer_initial(fast_tick_count());
    }
}

static WALL_CLOCK_INIT: Once = Once::new();

pub fn init() {
    let irq = crate::drivers::all_irq().first_unwrap();
    irq.apic_timer_enable();
    // RTC I/O ports (0x70/0x71) are not per-CPU — only the first caller reads
    // them to avoid concurrent port access corrupting the read under SMP.
    WALL_CLOCK_INIT.call_once(init_wall_clock_from_rtc);
}

// ---------------------------------------------------------------------------
// CMOS / MC146818 real-time clock
// ---------------------------------------------------------------------------
// Without this the wall clock starts at the Unix epoch (1970), which makes
// every TLS certificate look "not yet valid" and breaks `wget https://...`
// for any client that validates certificates. Reading the RTC at boot gives
// a real date so `date` is no longer required before HTTPS.

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

const RTC_SECONDS: u8 = 0x00;
const RTC_MINUTES: u8 = 0x02;
const RTC_HOURS: u8 = 0x04;
const RTC_DAY: u8 = 0x07;
const RTC_MONTH: u8 = 0x08;
const RTC_YEAR: u8 = 0x09;
const RTC_CENTURY: u8 = 0x32;
const RTC_STATUS_A: u8 = 0x0A;
const RTC_STATUS_B: u8 = 0x0B;

unsafe fn cmos_read(reg: u8) -> u8 {
    // Bit 7 of the index port controls NMI; keep it clear (NMI enabled).
    let mut addr = Port::<u8>::new(CMOS_ADDR);
    let mut data = Port::<u8>::new(CMOS_DATA);
    addr.write(reg & 0x7F);
    data.read()
}

unsafe fn rtc_update_in_progress() -> bool {
    cmos_read(RTC_STATUS_A) & 0x80 != 0
}

fn bcd_to_bin(v: u8) -> u8 {
    (v & 0x0F) + ((v >> 4) * 10)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RtcRaw {
    sec: u8,
    min: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
}

unsafe fn rtc_read_raw() -> RtcRaw {
    RtcRaw {
        sec: cmos_read(RTC_SECONDS),
        min: cmos_read(RTC_MINUTES),
        hour: cmos_read(RTC_HOURS),
        day: cmos_read(RTC_DAY),
        month: cmos_read(RTC_MONTH),
        year: cmos_read(RTC_YEAR),
        century: cmos_read(RTC_CENTURY),
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`). Valid for `year >= 1970`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Read the CMOS RTC and return seconds since the Unix epoch, or `None` if the
/// values look invalid (no RTC, garbage, etc.).
fn read_rtc_epoch() -> Option<u64> {
    unsafe {
        // Wait out any in-progress update, then read twice until two reads
        // agree, to avoid catching the RTC mid-tick.
        let mut spins = 0u32;
        while rtc_update_in_progress() {
            spins += 1;
            if spins > 1_000_000 {
                break;
            }
        }
        let mut last = rtc_read_raw();
        loop {
            let mut s = 0u32;
            while rtc_update_in_progress() {
                s += 1;
                if s > 1_000_000 {
                    break;
                }
            }
            let cur = rtc_read_raw();
            if cur == last {
                break;
            }
            last = cur;
        }

        let status_b = cmos_read(RTC_STATUS_B);
        let is_bcd = status_b & 0x04 == 0;
        let is_12h = status_b & 0x02 == 0;

        let mut sec = last.sec;
        let mut min = last.min;
        // Preserve the PM flag (bit 7) before any BCD conversion strips it.
        let pm = last.hour & 0x80 != 0;
        let mut hour = last.hour & 0x7F;
        let mut day = last.day;
        let mut month = last.month;
        let mut year = last.year;
        let mut century = last.century;

        if is_bcd {
            sec = bcd_to_bin(sec);
            min = bcd_to_bin(min);
            hour = bcd_to_bin(hour);
            day = bcd_to_bin(day);
            month = bcd_to_bin(month);
            year = bcd_to_bin(year);
            century = bcd_to_bin(century);
        }

        if is_12h {
            if pm {
                hour = (hour % 12) + 12;
            } else {
                hour %= 12;
            }
        }

        // The century register is optional; fall back to 21st century.
        let full_year: i64 = if (19..=21).contains(&century) {
            century as i64 * 100 + year as i64
        } else {
            2000 + year as i64
        };

        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || min > 59
            || sec > 60
            || full_year < 1970
        {
            return None;
        }

        let days = days_from_civil(full_year, month as i64, day as i64);
        if days < 0 {
            return None;
        }
        let secs = days as u64 * 86_400 + hour as u64 * 3_600 + min as u64 * 60 + sec as u64;
        Some(secs)
    }
}

fn init_wall_clock_from_rtc() {
    match read_rtc_epoch() {
        Some(epoch) => {
            crate::timer::wall_clock_set(Duration::from_secs(epoch));
            info!("wall clock initialized from RTC: {} s since epoch", epoch);
        }
        None => {
            warn!("RTC read failed; wall clock stays at boot epoch (1970)");
        }
    }
}

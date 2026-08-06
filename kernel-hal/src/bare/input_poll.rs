//! Input device poll hook for I/O wait loops and stdin read().

#[cfg(all(target_arch = "x86_64", not(feature = "no-pci"), feature = "xhci-usb-hid"))]
use core::sync::atomic::{AtomicU64, Ordering};

/// Minimum spacing between actual xHCI HID polls driven from an I/O-wait loop.
///
/// `poll_input_devices` is called on *every* future re-poll of an I/O wait —
/// `poll(2)`, `epoll_wait`, `read(stdin)`, a PTY read — and each such loop
/// re-arms itself on the 4 ms io-wait tick. A handful of shells parked in
/// `poll(stdin)` (spare VTs, seatd, udhcpc, a getty or two) therefore drove tens
/// of thousands of HID polls per second between them, and each poll is a burst
/// of xHCI MMIO reads — i.e. a burst of VM exits under emulation — which pegged
/// a host CPU core even with nothing happening on screen.
///
/// No USB HID device produces events faster than ~1 kHz (a 1000 Hz gaming mouse
/// is the fast extreme; keyboards report at 125 Hz), so collapsing the aggregate
/// io-wait poll rate to a 1 kHz ceiling keeps input latency imperceptible while
/// cutting the MMIO/VM-exit storm by ~30x. The periodic 125 Hz timer poll (see
/// `bare/timer.rs`) still runs independently as the idle-path backstop.
#[cfg(all(target_arch = "x86_64", not(feature = "no-pci"), feature = "xhci-usb-hid"))]
const IOWAIT_HID_POLL_INTERVAL_NS: u64 = 1_000_000; // 1 ms == 1 kHz

/// Monotonic time (ns) of the last io-wait-driven xHCI poll, shared across CPUs
/// so the ceiling bounds the *aggregate* rate no matter how many wait loops call
/// in. `0` means "never polled", which always lets the first call through.
#[cfg(all(target_arch = "x86_64", not(feature = "no-pci"), feature = "xhci-usb-hid"))]
static IOWAIT_HID_LAST_NS: AtomicU64 = AtomicU64::new(0);

pub fn poll_input_devices() {
    #[cfg(all(target_arch = "x86_64", not(feature = "no-pci")))]
    {
        #[cfg(feature = "xhci-usb-hid")]
        {
            // Rate-limit to ~1 kHz across all CPUs. The first caller to see the
            // interval elapse claims the slot via CAS and polls; everyone else
            // in that window returns immediately. This is what turns a
            // tens-of-thousands-per-second MMIO storm into a ~1 kHz trickle.
            let now = crate::timer::timer_now().as_nanos() as u64;
            let last = IOWAIT_HID_LAST_NS.load(Ordering::Relaxed);
            if now.wrapping_sub(last) < IOWAIT_HID_POLL_INTERVAL_NS {
                return;
            }
            if IOWAIT_HID_LAST_NS
                .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
                .is_err()
            {
                // Lost the race: another CPU just polled within this window.
                return;
            }
            crate::kstats::note_hid_poll_iowait(); // [diag]
            zcore_drivers::usb::xhci_hid::poll();
        }
    }
}

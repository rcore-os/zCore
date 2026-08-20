// Rust language features implementations

use core::alloc::Layout;
use core::panic::PanicInfo;

#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    // The heap is exhausted here, so we must NOT allocate: klog_*! use
    // `alloc::format!` and would recursively fail. Use the spin serial writer
    // (the same no-alloc path the panic handler uses) so the used/total numbers
    // actually reach the console — they pinpoint whether this is a leak.
    let heap_used = crate::memory::heap_used();
    let heap_total = crate::memory::heap_total();
    kernel_hal::console::serial_write_fmt_spin(format_args!(
        "\nkernel OOM: alloc {} bytes failed (used {} / total {} MiB)\n",
        layout.size(),
        heap_used / 1024 / 1024,
        heap_total / 1024 / 1024,
    ));
    // Attribution: live allocations per size class, so the OOM report says
    // WHICH class holds the heap (each line: class upper bound, live count,
    // total bytes if every allocation were at the bound).
    #[cfg(all(target_arch = "x86_64", not(feature = "libos")))]
    {
        let hist = crate::memory::heap_live_histogram();
        kernel_hal::console::serial_write_fmt_spin(format_args!("heap live by size class:\n"));
        for (i, count) in hist.iter().enumerate() {
            if *count > 0 {
                let size = 1usize << i;
                kernel_hal::console::serial_write_fmt_spin(format_args!(
                    "  <={:>10}B x {:<8} (~{} MiB)\n",
                    size,
                    count,
                    (count * size) >> 20,
                ));
            }
        }
        // memfd attribution: the 4 KiB blocks are ramfs file pages, and the
        // desktop's big page consumers are wl_shm pools backed by memfd.
        #[cfg(feature = "linux")]
        {
            let (created, live, bytes) = linux_object::fs::memfd_stats();
            kernel_hal::console::serial_write_fmt_spin(format_args!(
                "memfd: created={} live={} live_bytes={} MiB\n",
                created,
                live,
                bytes >> 20,
            ));
        }
        kernel_hal::console::serial_write_fmt_spin(format_args!("hot exact sizes:\n"));
        for (size, live) in crate::memory::heap_hot_sizes() {
            if size != 0 && live > 0 {
                kernel_hal::console::serial_write_fmt_spin(format_args!(
                    "  {}B x {} (~{} MiB)\n",
                    size,
                    live,
                    (size * live) >> 20,
                ));
            }
        }
    }
    panic!("memory allocation of {} bytes failed", layout.size());
}

/// Fixed-size, no-alloc formatter for the panic banner. The panic handler must
/// not allocate (the panic may BE an OOM) and must not depend on any lock.
struct StackBuf {
    buf: [u8; 768],
    len: usize,
}

impl core::fmt::Write for StackBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let room = self.buf.len() - self.len;
        let n = s.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

// ── Spinlock deadlock self-report ────────────────────────────────────────────
//
// Slots recording every CPU stuck >~8s on a spinlock (see kernel-sync's
// DEADLOCK_SPINS). The hook rebuilds a multi-line banner from ALL slots on
// each report, so a photo shows every stuck call site at once — both sides of
// an AB-BA deadlock, not just the last reporter. Lock-free by construction:
// atomics + the raw-framebuffer banner.
const DL_SLOTS: usize = 8;
static DL_FILE_PTR: [core::sync::atomic::AtomicUsize; DL_SLOTS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; DL_SLOTS];
static DL_FILE_LEN: [core::sync::atomic::AtomicUsize; DL_SLOTS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; DL_SLOTS];
static DL_LINE_CPU: [core::sync::atomic::AtomicUsize; DL_SLOTS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; DL_SLOTS];
/// 1 = this slot is the lock HOLDER's acquire site (reported by a waiter's
/// snapshot), 0 = a stuck waiter's own call site. The distinction is the whole
/// point: the waiters are usually innocent readers; the holder line is the one
/// that names the wedged code path.
static DL_HOLDER: [core::sync::atomic::AtomicUsize; DL_SLOTS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; DL_SLOTS];

/// Record one `(site, cpu, role)` into the slots (deduplicated) — lock-free.
fn dl_record(ptr: usize, len: usize, line: u32, cpu: u32, holder: bool) {
    use core::sync::atomic::Ordering;
    for i in 0..DL_SLOTS {
        let cur = DL_FILE_PTR[i].load(Ordering::SeqCst);
        if cur == ptr
            && (DL_LINE_CPU[i].load(Ordering::SeqCst) as u32) == line
            && (DL_HOLDER[i].load(Ordering::SeqCst) != 0) == holder
        {
            return;
        }
        if cur == 0
            && DL_FILE_PTR[i]
                .compare_exchange(0, ptr, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            DL_FILE_LEN[i].store(len, Ordering::SeqCst);
            DL_LINE_CPU[i].store(((cpu as usize) << 32) | line as usize, Ordering::SeqCst);
            DL_HOLDER[i].store(holder as usize, Ordering::SeqCst);
            return;
        }
    }
}

/// Rebuild and paint the banner from all recorded slots.
fn dl_paint() {
    use core::fmt::Write;
    use core::sync::atomic::Ordering;
    let mut b = StackBuf {
        buf: [0u8; 768],
        len: 0,
    };
    let _ = write!(b, "DEADLOCK: spinlock(s) stuck >8s");
    // Track whether any HOLDER is itself blocked in a TLB-shootdown ack-wait:
    // that is the "shootdown starvation" signature (convoy behind one CPU that
    // is waiting on a peer that never acks — a non-pumping IRQs-off spinner),
    // as opposed to a true lock-ordering AB-BA cycle.
    let mut shootdown_head = false;
    for i in 0..DL_SLOTS {
        let p = DL_FILE_PTR[i].load(Ordering::SeqCst);
        if p == 0 {
            continue;
        }
        let l = DL_FILE_LEN[i].load(Ordering::SeqCst);
        let lc = DL_LINE_CPU[i].load(Ordering::SeqCst);
        let cpu = (lc >> 32) as usize;
        let is_holder = DL_HOLDER[i].load(Ordering::SeqCst) != 0;
        let role = if is_holder { "HOLDER " } else { "" };
        // SAFETY: (p, l) were stored from a live &'static str (either the
        // reporter's own #[track_caller] file, or the holder's, snapshotted by
        // kernel-sync from the same immortal strings).
        let f = unsafe {
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(p as *const u8, l))
        };
        let _ = write!(b, "\n{}cpu={} at {}:{}", role, cpu, f, lc & 0xffff_ffff);
        // If this CPU is spin-waiting for a TLB-shootdown ack, name the CPUs it
        // is blocked on. A HOLDER shown here is the convoy head — the machine is
        // wedged not by a lock cycle but because those CPUs never acked.
        let mask = kernel_hal::shootdown_wait_mask(cpu);
        if mask != 0 {
            if is_holder {
                shootdown_head = true;
            }
            let _ = write!(b, " [TLB-ack wait, blocked on cpu");
            let mut m = mask;
            let mut first = true;
            while m != 0 {
                let c = m.trailing_zeros();
                let _ = write!(b, "{}{}", if first { " " } else { "," }, c);
                first = false;
                m &= m - 1;
            }
            let _ = write!(b, "]");
        }
    }
    // One-line verdict so the on-screen (no-serial) capture is self-diagnosing.
    if shootdown_head {
        let _ = write!(
            b,
            "\nDIAG: shootdown starvation — the HOLDER waits a TLB ack from a CPU \
             that never pumps (likely deep in vendor RM IRQs-off code); not AB-BA."
        );
    } else {
        let _ = write!(
            b,
            "\nDIAG: no HOLDER is in a shootdown wait -> lock-ordering cycle (AB-BA); \
             compare the HOLDER site(s) above."
        );
    }
    let valid = match core::str::from_utf8(&b.buf[..b.len]) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(&b.buf[..e.valid_up_to()]).unwrap_or(""),
    };
    kernel_hal::console::panic_banner(valid);
    // …and to the serial console, which is the only one anybody watching a
    // headless run can see.
    //
    // `panic_banner` above rasterizes straight onto the framebuffer, and its
    // x86_64 implementation is `#[cfg(feature = "graphic")]` — so on a build
    // without graphics, driven over `-serial mon:stdio` with `-display none`
    // (which is exactly how the QEMU benchmark harness runs), the deadlock
    // detector was a complete no-op. A silent hang was therefore indistinguishable
    // from a hang with a fired-but-invisible deadlock report, and "no banner
    // appeared" got mistaken for "not a deadlock". It is not evidence unless it
    // can reach the observer.
    //
    // Spin rather than `try_lock`: by the time this fires the machine has been
    // wedged for eight seconds and a dropped report is a wasted debugging cycle.
    // The deadlock hook runs from inside a stuck lock acquisition, where
    // `push_off` has already disabled interrupts — the precondition
    // `serial_write_fmt_spin` documents.
    //
    // Re-emitted whenever the banner GAINS content, not once. The first report
    // is the waiter's, and its serial line goes out before the same waiter has
    // snapshotted the HOLDER — so a once-guard mailed out half the diagnosis
    // and permanently suppressed the line that names the culprit. On a
    // graphics-less run (the benchmark harness) that made the holder
    // unknowable: the framebuffer repaint had it, and nobody could see it.
    // Slot count bounds the reprints (each unique site is recorded once), so
    // this cannot storm: at most DL_SLOTS emissions ever.
    static DL_SERIAL_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let prev = DL_SERIAL_LEN.load(Ordering::SeqCst);
    if b.len > prev
        && DL_SERIAL_LEN
            .compare_exchange(prev, b.len, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        kernel_hal::console::serial_write_fmt_spin(format_args!("\n[{}]\n", valid));
    }
}

pub fn deadlock_report(file: &'static str, line: u32) {
    let cpu = kernel_hal::cpu::cpu_id() as u32;
    dl_record(file.as_ptr() as usize, file.len(), line, cpu, false);
    dl_paint();
}

/// Twin of [`deadlock_report`] for the lock HOLDER, called by a stuck waiter
/// with the holder's acquire site snapshotted from the lock itself (see
/// kernel-sync's `set_deadlock_holder_hook`). `cpu` is the cpu that acquired
/// the lock, not the reporter's.
pub fn deadlock_holder_report(file_ptr: usize, file_len: usize, line: u32, cpu: u32) {
    dl_record(file_ptr, file_len, line, cpu, true);
    dl_paint();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Disable interrupts immediately. With panic-strategy=abort, local variables
    // in the panicking function (e.g. kernel-sync's RefMut borrow guard in
    // pop_off) are never dropped. If a timer IRQ fires while the panic handler
    // is running, push_off/pop_off will call borrow_mut() on an already-borrowed
    // RefCell → nested panic → abort() → ud2 → triple fault → QEMU reset.
    kernel_hal::interrupt::intr_off();

    // FIRST, before anything that touches a lock: rasterize the panic straight
    // onto the framebuffer (red band, raw pixel writes, no locks, no alloc).
    // Everything below can be silently dropped or deadlock when another CPU —
    // or THIS one — holds the console/serial locks (a panic inside an IRQ
    // handler mid-print left the screen frozen half-line with the real panic
    // visible only on serial). This banner cannot.
    {
        use core::fmt::Write;
        let mut b = StackBuf {
            buf: [0u8; 768],
            len: 0,
        };
        if let Some(loc) = info.location() {
            let _ = write!(
                b,
                "KERNEL PANIC cpu={} {}:{}\n{}",
                kernel_hal::cpu::cpu_id(),
                loc.file(),
                loc.line(),
                info.message()
            );
        } else {
            let _ = write!(
                b,
                "KERNEL PANIC cpu={}\n{}",
                kernel_hal::cpu::cpu_id(),
                info.message()
            );
        }
        let valid = match core::str::from_utf8(&b.buf[..b.len]) {
            Ok(s) => s,
            // Truncation can split a multi-byte char; keep the valid prefix.
            Err(e) => core::str::from_utf8(&b.buf[..e.valid_up_to()]).unwrap_or(""),
        };
        kernel_hal::console::panic_banner(valid);
    }

    // Make the panic VISIBLE after a compositor took the screen. Once labwc
    // sets KD_GRAPHICS the kernel stops PRESENTING the text console (writes
    // still land in the shadow buffer but are never pushed to the display), so
    // a panic here would only reach serial — the monitor stays black on the
    // compositor's last frame and the crash reads as a silent freeze. Forcing
    // the active VT back to KD_TEXT repaints the text console and makes every
    // graphic_console_write_fmt below actually appear on the monitor. It is
    // panic-safe: the repaint is best-effort try_lock and allocates nothing.
    //
    // Remembered, not just set: if `oops` manages to contain this fault the
    // machine keeps running, and leaving the VT in KD_TEXT would strand a live
    // compositor rendering into a buffer that is no longer presented — the
    // desktop would look frozen even though nothing but one process died.
    let prev_kd = kernel_hal::console::kd_mode();
    kernel_hal::console::set_kd_mode(kernel_hal::console::KD_TEXT);

    // Use spin variant: interrupts are already off above, and try_lock silently
    // discards output if another CPU holds the lock — unacceptable in panic context.
    //
    // Mirror to the graphic console too: on a real bring-up box with only a
    // monitor (no serial capture), a serial-only panic is invisible and reads
    // as a silent freeze. graphic_console_write_fmt is a best-effort try_lock
    // that no-ops if the VT lock is held, so it can't deadlock the panic path.
    if let Some(loc) = info.location() {
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "\n\npanic cpu={} at {}:{}:{}\n",
            kernel_hal::cpu::cpu_id(),
            loc.file(),
            loc.line(),
            loc.column(),
        ));
        kernel_hal::console::graphic_console_write_fmt_spin(format_args!(
            "\n\n[PANIC] cpu={} at {}:{}:{}\n",
            kernel_hal::cpu::cpu_id(),
            loc.file(),
            loc.line(),
            loc.column(),
        ));
    } else {
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "\n\npanic cpu={}\n",
            kernel_hal::cpu::cpu_id(),
        ));
        kernel_hal::console::graphic_console_write_fmt_spin(format_args!(
            "\n\n[PANIC] cpu={}\n",
            kernel_hal::cpu::cpu_id(),
        ));
    }
    // `as_str()` returns None for any panic! with format arguments — use
    // Display on the Arguments directly so the message is always printed.
    kernel_hal::console::serial_write_fmt_spin(format_args!("{}\n", info.message()));
    kernel_hal::console::graphic_console_write_fmt_spin(format_args!("{}\n", info.message()));

    // Frame-pointer backtrace: walk the saved RBP chain and print return
    // addresses so a panic in an inlined helper (e.g. x86_64::VirtAddr::new
    // called from deep in the paging code) can be mapped back to the real
    // caller with `nm`/`addr2line`. Best-effort and bounded: stop on a null /
    // unaligned / non-canonical frame pointer so a corrupt chain can't fault
    // the panic handler itself.
    #[cfg(target_arch = "x86_64")]
    {
        let mut rbp: usize;
        unsafe { core::arch::asm!("mov {}, rbp", out(reg) rbp) };
        // Mirror the backtrace to BOTH serial and the graphic console. On a
        // headless-but-monitor'd bring-up box (the real-HW case) the operator
        // only ever sees the graphic framebuffer -- serial-only backtraces are
        // invisible in a phone photo of the screen, which is the only artifact
        // that comes back. Printing the return addresses on the framebuffer too
        // means a photo of the panic is enough to `addr2line` the culprit.
        kernel_hal::console::serial_write_fmt_spin(format_args!("[backtrace]\n"));
        kernel_hal::console::graphic_console_write_fmt_spin(format_args!("[backtrace]\n"));
        for _ in 0..32 {
            if rbp == 0 || rbp & 0x7 != 0 || rbp < 0xffff_8000_0000_0000 {
                break;
            }
            let next = unsafe { core::ptr::read_volatile(rbp as *const usize) };
            let ret = unsafe { core::ptr::read_volatile((rbp + 8) as *const usize) };
            if ret == 0 {
                break;
            }
            kernel_hal::console::serial_write_fmt_spin(format_args!("  ret={:#x}\n", ret));
            kernel_hal::console::graphic_console_write_fmt_spin(format_args!("  ret={:#x}\n", ret));
            if next <= rbp {
                break; // stack grows down; a non-increasing frame is corrupt
            }
            rbp = next;
        }
    }

    // How many kernel faults this boot has already survived. A panic arriving
    // behind others that were contained is usually the same root cause coming
    // back, and that is worth knowing from the banner alone.
    let contained = crate::oops::contained_count();
    if contained > 0 {
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "[oops] kernel faults already contained this boot: {}\n",
            contained
        ));
    }

    // Last resort before halting: if this panic happened while serving one
    // particular task -- and only then -- kill that task and hand the CPU back
    // to the scheduler instead of taking the whole system down. Does not return
    // if it succeeds; if it returns it has already said why it could not, and
    // we halt as always. Not attempted under `baremetal-test`, where a panic
    // *must* end the machine so the test fails.
    if !cfg!(feature = "baremetal-test") {
        crate::oops::try_contain("kernel panic", Some(prev_kd));
    }

    if cfg!(feature = "baremetal-test") {
        kernel_hal::cpu::reset();
    } else {
        loop {
            core::hint::spin_loop();
        }
    }
}

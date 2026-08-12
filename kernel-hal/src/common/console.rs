//! Console input and output.

use crate::drivers;
use core::fmt::{Arguments, Result, Write};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Kernel log (dmesg) callback
// ---------------------------------------------------------------------------
// The `zcore` crate owns the actual ring buffer; it registers function
// pointers here so that `linux-syscall` can call `klog_read` / `klog_buf_size`
// without a direct crate dependency on `zcore`.

static KLOG_READ_FN: AtomicUsize = AtomicUsize::new(0);
static KLOG_SIZE_FN: AtomicUsize = AtomicUsize::new(0);
static KLOG_EMIT_FN: AtomicUsize = AtomicUsize::new(0);

/// Called once by `zcore` at startup to register the ring-buffer accessors.
pub fn klog_register(
    read_fn: fn(&mut [u8]) -> usize,
    size_fn: fn() -> usize,
    emit_fn: fn(u8, &str),
) {
    KLOG_READ_FN.store(read_fn as usize, Ordering::SeqCst);
    KLOG_SIZE_FN.store(size_fn as usize, Ordering::SeqCst);
    KLOG_EMIT_FN.store(emit_fn as usize, Ordering::SeqCst);
}

/// Copy the kernel log ring buffer into `dst`.  Returns bytes written.
/// Returns 0 if no callback has been registered yet.
pub fn klog_read(dst: &mut [u8]) -> usize {
    let p = KLOG_READ_FN.load(Ordering::SeqCst);
    if p == 0 {
        return 0;
    }
    let f: fn(&mut [u8]) -> usize = unsafe { core::mem::transmute(p) };
    f(dst)
}

/// Total bytes currently stored in the kernel log ring buffer.
pub fn klog_buf_size() -> usize {
    let p = KLOG_SIZE_FN.load(Ordering::SeqCst);
    if p == 0 {
        return 0;
    }
    let f: fn() -> usize = unsafe { core::mem::transmute(p) };
    f()
}

/// Syslog priorities (Linux `syslog.h`).
pub const LOG_ERR: u8 = 3;
pub const LOG_WARNING: u8 = 4;
pub const LOG_INFO: u8 = 6;

/// Append a vital kernel message to the dmesg ring buffer (syslog priority 0–7).
/// Always recorded regardless of the `log` crate max level.
pub fn klog_emit(priority: u8, msg: &str) {
    let p = KLOG_EMIT_FN.load(Ordering::SeqCst);
    if p == 0 {
        return;
    }
    let f: fn(u8, &str) = unsafe { core::mem::transmute(p) };
    f(priority, msg);
}

struct SerialWriter;

static SERIAL_WRITER: spin::Mutex<SerialWriter> = spin::Mutex::new(SerialWriter);

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> Result {
        if let Some(uart) = drivers::all_uart().first() {
            uart.write_str(s).unwrap();
            #[cfg(feature = "graphic")]
            if GRAPHIC_VTS.try_get().is_none() {
                crate::hal_fn::console::console_write_early(s);
            }
        } else {
            crate::hal_fn::console::console_write_early(s);
        }
        Ok(())
    }
}

struct DebugWriter;

static DEBUG_WRITER: spin::Mutex<DebugWriter> = spin::Mutex::new(DebugWriter);

impl Write for DebugWriter {
    fn write_str(&mut self, s: &str) -> Result {
        crate::hal_fn::console::console_write_early(s);
        Ok(())
    }
}

cfg_if! {
    if #[cfg(feature = "graphic")] {
        use crate::utils::init_once::InitOnce;
        use alloc::sync::Arc;
        use zcore_drivers::{scheme::DisplayScheme, utils::GraphicConsole};

        use alloc::vec::Vec;

        static GRAPHIC_VTS: InitOnce<Vec<spin::Mutex<GraphicConsole>>> = InitOnce::new();
        static CONSOLE_WIN_SIZE: InitOnce<ConsoleWinSize> = InitOnce::new();
        static GRAPHIC_DISPLAY: InitOnce<Arc<dyn DisplayScheme>> = InitOnce::new();
        static ACTIVE_VT: AtomicUsize = AtomicUsize::new(0);
        static CLEAR_ON_NEXT_GRAPHIC_WRITE: AtomicBool = AtomicBool::new(false);

        pub(crate) fn init_graphic_console(display: Arc<dyn DisplayScheme>) {
            let info = display.info();
            GRAPHIC_DISPLAY.init_once_by(display.clone());
            let mut vts = Vec::with_capacity(NUM_VTS);
            let mut winsz = ConsoleWinSize::default();
            for i in 0..NUM_VTS {
                let cons = GraphicConsole::new(display.clone());
                if i == 0 {
                    winsz = ConsoleWinSize {
                        ws_row: cons.rows() as u16,
                        ws_col: cons.columns() as u16,
                        ws_xpixel: info.width as u16,
                        ws_ypixel: info.height as u16,
                    };
                }
                vts.push(spin::Mutex::new(cons));
            }
            CONSOLE_WIN_SIZE.init_once_by(winsz);
            GRAPHIC_VTS.init_once_by(vts);
            // Make boot UX robust on real hardware: clear once on first graphic write
            // even if userspace/loader ordering differs.
            CLEAR_ON_NEXT_GRAPHIC_WRITE.store(true, Ordering::SeqCst);
        }

        fn vt_mutex(n: usize) -> Option<&'static spin::Mutex<GraphicConsole>> {
            GRAPHIC_VTS.try_get().and_then(|v| v.get(n))
        }

        /// Present coalescing for write-driven console output.
        ///
        /// A scroll redraws the whole shadow buffer, so presenting after every
        /// `write` turned line-by-line output (`cat`, build logs — stdio splits
        /// its buffer at each `\n`) into one full-screen blit per line. Writes
        /// inside the window only latch [`PRESENT_PENDING`]; the 250 Hz timer
        /// tick flushes the tail via [`flush_pending_present`], so a burst
        /// coalesces to at most ~60 presents/s and the final line still reaches
        /// the screen within one tick (≤4 ms). An isolated write (interactive
        /// echo) is past the window and presents immediately, so key-echo
        /// latency is unchanged.
        const PRESENT_MIN_INTERVAL_NS: u64 = 16_000_000; // ~60 Hz
        // Qualified (not imported at the top): this block is graphic-gated,
        // and the bare import broke `deny(warnings)` in non-graphic builds.
        static LAST_PRESENT_NS: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        static PRESENT_PENDING: AtomicBool = AtomicBool::new(false);

        fn present_throttled(g: &mut GraphicConsole) {
            let now = crate::hal_fn::timer::timer_now().as_nanos() as u64;
            let last = LAST_PRESENT_NS.load(Ordering::Relaxed);
            if now.wrapping_sub(last) >= PRESENT_MIN_INTERVAL_NS {
                LAST_PRESENT_NS.store(now, Ordering::Relaxed);
                PRESENT_PENDING.store(false, Ordering::Release);
                g.present();
            } else {
                PRESENT_PENDING.store(true, Ordering::Release);
            }
        }

        /// Timer-tick side of the coalescer: push a deferred present once the
        /// throttle window has elapsed. `try_lock` only — this runs in IRQ
        /// context and a busy console simply retries next tick.
        pub(crate) fn flush_pending_present() {
            if !PRESENT_PENDING.load(Ordering::Acquire) {
                return;
            }
            let vt = ACTIVE_VT.load(Ordering::SeqCst);
            if !present_allowed(vt) {
                // Userspace took the framebuffer while a present was pending;
                // drop it (KD_TEXT re-entry repaints the whole VT anyway).
                PRESENT_PENDING.store(false, Ordering::Release);
                return;
            }
            let now = crate::hal_fn::timer::timer_now().as_nanos() as u64;
            let last = LAST_PRESENT_NS.load(Ordering::Relaxed);
            if now.wrapping_sub(last) < PRESENT_MIN_INTERVAL_NS {
                return;
            }
            if let Some(cons) = vt_mutex(vt) {
                if let Some(mut g) = cons.try_lock() {
                    LAST_PRESENT_NS.store(now, Ordering::Relaxed);
                    PRESENT_PENDING.store(false, Ordering::Release);
                    g.present();
                }
            }
        }

        /// Request a one-shot clear-to-black of the graphic console before the next write.
        pub fn request_clear_graphic_on_next_write() {
            // Finalize the boot progress indicator before switching to a cleared
            // native graphic console.
            crate::hal_fn::console::console_progress_early(100);
            CLEAR_ON_NEXT_GRAPHIC_WRITE.store(true, Ordering::SeqCst);
        }

        fn maybe_clear_graphic_before_write(vt: usize) {
            if !CLEAR_ON_NEXT_GRAPHIC_WRITE.swap(false, Ordering::SeqCst) {
                return;
            }
            if let (Some(display), Some(cons)) = (GRAPHIC_DISPLAY.try_get(), vt_mutex(vt)) {
                // try_lock, NOT lock: this was the console path's only BLOCKING
                // uninstrumented spin::Mutex acquisition, reachable from any
                // logging context (incl. IRQ) — a wedged holder turned it into
                // an invisible infinite spin. On contention, re-arm the flag so
                // the clear happens on the next write instead.
                if let Some(mut g) = cons.try_lock() {
                    // Clear to black with opaque alpha (ARGB8888) and reset the console state.
                    let _ = crate::boot_logo::clear_screen(
                        &**display,
                        zcore_drivers::prelude::RgbColor::new(0, 0, 0),
                    );
                    *g = GraphicConsole::new(display.clone());
                } else {
                    CLEAR_ON_NEXT_GRAPHIC_WRITE.store(true, Ordering::SeqCst);
                }
            }
        }

        /// Write to a specific VT's console buffer. The pixels are only pushed to
        /// the display when this is the active VT and we are in text mode;
        /// background VTs keep accumulating in their own shadow buffer.
        pub(crate) fn vt_write_str_impl(vt: usize, s: &str) {
            let active = vt == ACTIVE_VT.load(Ordering::SeqCst);
            if active {
                maybe_clear_graphic_before_write(vt);
            }
            if let Some(cons) = vt_mutex(vt) {
                if let Some(mut g) = cons.try_lock() {
                    let _ = g.write_str(s);
                    if active && present_allowed(vt) {
                        present_throttled(&mut g);
                    }
                }
            }
        }

        /// Panic-path write: the normal path `try_lock`s and silently DROPS the
        /// write when another CPU holds the VT console lock (it is mid-print of
        /// its own line). A panic message must not be droppable — that exact
        /// race produced a screen frozen mid-line with the real panic visible
        /// only on serial. Spin (bounded) for the lock instead: a live holder
        /// releases within microseconds; the bound keeps a wedged holder from
        /// turning the panic handler itself into a hang.
        pub(crate) fn vt_write_fmt_spin_impl(vt: usize, fmt: Arguments) {
            if let Some(cons) = vt_mutex(vt) {
                for _ in 0..50_000_000u64 {
                    if let Some(mut g) = cons.try_lock() {
                        let _ = g.write_fmt(fmt);
                        g.present();
                        return;
                    }
                    core::hint::spin_loop();
                }
            }
        }

        pub(crate) fn vt_write_fmt_impl(vt: usize, fmt: Arguments) {
            let active = vt == ACTIVE_VT.load(Ordering::SeqCst);
            if active {
                maybe_clear_graphic_before_write(vt);
            }
            if let Some(cons) = vt_mutex(vt) {
                if let Some(mut g) = cons.try_lock() {
                    let _ = g.write_fmt(fmt);
                    if active && present_allowed(vt) {
                        present_throttled(&mut g);
                    }
                }
            }
        }

        /// Make VT `n` the active one and repaint it to the display.
        pub(crate) fn switch_vt_impl(n: usize) {
            if let Some(v) = GRAPHIC_VTS.try_get() {
                if n >= v.len() {
                    return;
                }
                ACTIVE_VT.store(n, Ordering::SeqCst);
                if kd_mode_vt(n) == KD_TEXT {
                    if let Some(mut g) = v[n].try_lock() {
                        g.repaint();
                    }
                }
            }
        }

        pub(crate) fn scroll_active_vt(direction: i32) {
            if let Some(cons) = vt_mutex(ACTIVE_VT.load(Ordering::SeqCst)) {
                if let Some(mut g) = cons.try_lock() {
                    g.buf_mut().scroll_history(direction);
                    g.present();
                }
            }
        }

        pub(crate) fn blink_active_vt(visible: bool) {
            let vt = ACTIVE_VT.load(Ordering::SeqCst);
            // labwc/KD_GRAPHICS owns the FB: never call into DisplayScheme from
            // the timer path (present_with_cursor → dyn blit/flush). A half-
            // torn-down or compositor-owned display was a null-vtable EXECUTE
            // vector with `in_timer_callback` previously unset.
            if !present_allowed(vt) {
                return;
            }
            if let Some(cons) = vt_mutex(vt) {
                if let Some(mut g) = cons.try_lock() {
                    g.set_cursor_blink(visible);
                }
            }
        }

        /// Repaint the active VT from its backing buffer.
        ///
        /// Used when returning from `KD_GRAPHICS` to `KD_TEXT`: a userspace
        /// graphics server may have overwritten the framebuffer.
        pub(crate) fn redraw_graphic_console_impl() {
            if let Some(cons) = vt_mutex(ACTIVE_VT.load(Ordering::SeqCst)) {
                if let Some(mut g) = cons.try_lock() {
                    g.repaint();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// KD console mode (Linux VT `KD_SETMODE` / `KD_GETMODE` semantics)
// ---------------------------------------------------------------------------
// In `KD_GRAPHICS` the kernel stops drawing the text console so a userspace
// graphics server (X/Wayland/DRM client) can own the framebuffer. Switching
// back to `KD_TEXT` repaints the text console.

/// Text mode: the kernel owns and draws the framebuffer console.
pub const KD_TEXT: u32 = 0x00;
/// Graphics mode: userspace owns the framebuffer; the console stops drawing.
pub const KD_GRAPHICS: u32 = 0x01;

// KD mode is per-VT (like Linux): an X server putting *its* VT into
// `KD_GRAPHICS` must not stop the kernel drawing the other text consoles, so
// switching away from the graphics VT still shows a normal text terminal.
static KD_MODES: [AtomicU32; NUM_VTS] = [const { AtomicU32::new(KD_TEXT) }; NUM_VTS];

/// Diagnostic: when true, kernel text-console writes are PRESENTED to the
/// display even while a userspace compositor holds the VT in `KD_GRAPHICS`.
/// Normally KD_GRAPHICS suppresses presentation so labwc owns the screen, but
/// that also hides a hard hang's last kernel log line on a monitor-only box.
/// With this on, the console log stays visible over the compositor, so the last
/// message before a freeze is frozen on screen.
static DIAG_PRESENT_OVER_GRAPHICS: AtomicBool = AtomicBool::new(false);

/// Enable/disable presenting kernel console output over a KD_GRAPHICS VT.
pub fn set_diag_present_over_graphics(on: bool) {
    DIAG_PRESENT_OVER_GRAPHICS.store(on, Ordering::Relaxed);
}

/// Whether a write to VT `vt` should be pushed to the display now: always in
/// text mode, and in graphics mode only when the diagnostic override is on.
#[inline]
#[allow(dead_code)]
fn present_allowed(vt: usize) -> bool {
    kd_mode_vt(vt) == KD_TEXT || DIAG_PRESENT_OVER_GRAPHICS.load(Ordering::Relaxed)
}

/// Set the KD mode of a specific VT (`KD_TEXT` or `KD_GRAPHICS`).
pub fn set_kd_mode_vt(vt: usize, mode: u32) {
    if let Some(m) = KD_MODES.get(vt) {
        m.store(mode, Ordering::SeqCst);
    }
    #[cfg(feature = "graphic")]
    if mode == KD_TEXT && vt == active_vt() {
        redraw_graphic_console_impl();
    }
}

/// Get the KD mode of a specific VT.
pub fn kd_mode_vt(vt: usize) -> u32 {
    KD_MODES
        .get(vt)
        .map(|m| m.load(Ordering::SeqCst))
        .unwrap_or(KD_TEXT)
}

/// Set the KD mode of the currently active VT.
pub fn set_kd_mode(mode: u32) {
    set_kd_mode_vt(active_vt(), mode);
}

/// Get the KD mode of the currently active VT.
pub fn kd_mode() -> u32 {
    kd_mode_vt(active_vt())
}

// ---------------------------------------------------------------------------
// Virtual terminals (VT) — Linux-style tty1..ttyN multiplexed on one display
// ---------------------------------------------------------------------------

/// Number of virtual terminals (Linux-style `tty1..ttyN`).
pub const NUM_VTS: usize = 6;

/// Number of virtual terminals available.
pub fn num_vts() -> usize {
    #[cfg(feature = "graphic")]
    {
        return GRAPHIC_VTS.try_get().map(|v| v.len()).unwrap_or(1);
    }
    #[cfg(not(feature = "graphic"))]
    {
        1
    }
}

/// Index of the currently active VT.
pub fn active_vt() -> usize {
    #[cfg(feature = "graphic")]
    {
        return ACTIVE_VT.load(Ordering::SeqCst);
    }
    #[cfg(not(feature = "graphic"))]
    {
        0
    }
}

/// Make VT `n` the active one and repaint it to the display.
#[allow(unused_variables)]
pub fn switch_vt(n: usize) {
    #[cfg(feature = "graphic")]
    switch_vt_impl(n);
}

/// Write a string into a specific VT's graphic console.
#[allow(unused_variables)]
pub fn vt_write_str(vt: usize, s: &str) {
    #[cfg(feature = "graphic")]
    vt_write_str_impl(vt, s);
}

/// Write formatted data into a specific VT's graphic console.
#[allow(unused_variables)]
pub fn vt_write_fmt(vt: usize, fmt: Arguments) {
    #[cfg(feature = "graphic")]
    vt_write_fmt_impl(vt, fmt);
}

/// Write a string to VT `vt`: always to its graphic console, and to the serial
/// port when `vt` is the active terminal (so the serial log mirrors the screen).
pub fn vt_console_write_str(vt: usize, s: &str) {
    if vt == active_vt() {
        serial_write_str(s);
    }
    vt_write_str(vt, s);
}

/// Blink the graphic-console text cursor.
///
/// Invoked from the timer tick (~250 Hz). It rate-limits itself to a ~2 Hz
/// blink using the monotonic clock and only does work when the blink phase
/// actually flips, so the common tick is just one atomic load. A no-op when the
/// `graphic` feature is disabled or while in `KD_GRAPHICS`.
pub fn cursor_blink_tick() {
    #[cfg(feature = "graphic")]
    {
        // No graphic consoles yet: do not touch DisplayScheme from IRQ/timer
        // context.
        if GRAPHIC_VTS.try_get().is_none() {
            return;
        }
        // Push any present deferred by the write-path coalescer (it checks
        // `present_allowed` itself, and drops the pending flag when userspace
        // owns the framebuffer).
        flush_pending_present();
        if !present_allowed(active_vt()) {
            return;
        }
        static LAST_PHASE: AtomicUsize = AtomicUsize::new(usize::MAX);
        let ms = crate::hal_fn::timer::timer_now().as_millis() as usize;
        let phase = (ms / 500) & 1;
        if LAST_PHASE.swap(phase, Ordering::SeqCst) == phase {
            return;
        }
        blink_active_vt(phase == 0);
    }
}

/// Request a one-shot clear-to-black of the graphic console before the next write.
///
/// When `feature="graphic"` is disabled, this is a no-op.
#[cfg(not(feature = "graphic"))]
pub fn request_clear_graphic_on_next_write() {
    crate::hal_fn::console::console_progress_early(100);
}

/// Writes a string slice into the serial.
pub fn serial_write_str(s: &str) {
    if let Some(mut w) = SERIAL_WRITER.try_lock() {
        let _ = w.write_str(s);
    }
}

/// Writes formatted data into the serial.
pub fn serial_write_fmt(fmt: Arguments) {
    if let Some(mut w) = SERIAL_WRITER.try_lock() {
        let _ = w.write_fmt(fmt);
    }
}

/// Writes formatted data into the serial, spinning until the lock is free.
///
/// Use in panic/abort context where dropping output silently is unacceptable.
/// Caller must ensure interrupts are disabled to avoid deadlock on the same CPU.
pub fn serial_write_fmt_spin(fmt: Arguments) {
    let _ = SERIAL_WRITER.lock().write_fmt(fmt);
}

/// Writes a string slice into the serial through sbi call.
pub fn debug_write_str(s: &str) {
    if let Some(mut w) = DEBUG_WRITER.try_lock() {
        let _ = w.write_str(s);
    }
}

/// Writes formatted data into the serial through sbi call..
pub fn debug_write_fmt(fmt: Arguments) {
    if let Some(mut w) = DEBUG_WRITER.try_lock() {
        let _ = w.write_fmt(fmt);
    }
}

/// Draw a boot progress bar on the early framebuffer console (UEFI GOP), if available.
///
/// This is intended for very early boot stages before the native graphic driver exists.
pub fn early_progress_bar(progress: u32) {
    crate::hal_fn::console::console_progress_early(progress);
}

/// Scrolls the graphic console history up (direction > 0) or down (direction < 0).
#[allow(unused_variables)]
pub fn scroll_graphic_console(direction: i32) {
    #[cfg(feature = "graphic")]
    scroll_active_vt(direction);
}

/// Writes a string slice into the graphic console.
#[allow(unused_variables)]
pub fn graphic_console_write_str(s: &str) {
    #[cfg(feature = "graphic")]
    vt_write_str_impl(active_vt(), s);
}

/// Writes formatted data into the graphic console.
#[allow(unused_variables)]
pub fn graphic_console_write_fmt(fmt: Arguments) {
    #[cfg(feature = "graphic")]
    vt_write_fmt_impl(active_vt(), fmt);
}

/// Panic-path graphic write: bounded-spins for the VT console lock instead of
/// silently dropping the message when another CPU is mid-print (see
/// `vt_write_fmt_spin_impl`). Use ONLY from the panic handler.
#[allow(unused_variables)]
pub fn graphic_console_write_fmt_spin(fmt: Arguments) {
    #[cfg(feature = "graphic")]
    vt_write_fmt_spin_impl(active_vt(), fmt);
}

/// Absolute last-resort panic output: rasterize `s` onto a red band at the top
/// of the framebuffer with raw pixel writes — no locks, no RefCell, no
/// allocation. Works even when every console lock is wedged (e.g. a panic
/// inside an IRQ handler while another CPU holds the VT/serial locks). No-op on
/// targets with no direct framebuffer.
pub fn panic_banner(s: &str) {
    crate::hal_fn::console::console_panic_banner(s);
}

/// Writes a string slice into the serial, and the graphic console if it exists.
pub fn console_write_str(s: &str) {
    serial_write_str(s);
    graphic_console_write_str(s);
}

/// Writes formatted data into the serial, and the graphic console if it exists.
pub fn console_write_fmt(fmt: Arguments) {
    serial_write_fmt(fmt);
    graphic_console_write_fmt(fmt);
}

/// Read buffer data from console (serial).
pub async fn console_read(buf: &mut [u8]) -> usize {
    super::future::SerialReadFuture::new(buf).await
}

/// The POSIX `winsize` structure.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ConsoleWinSize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

/// A userspace-supplied window size (via `TIOCSWINSZ`) that overrides the
/// framebuffer-derived one. The framebuffer console is huge (e.g. 227x113 at a
/// 2048x2048 mode), which is right for the on-screen graphic console but wrong
/// for a *serial* viewer whose terminal window is much smaller: full-screen
/// apps (nano, less, top) then lay out for 227x113 and overflow, wrapping their
/// status/help lines into garbage. A serial login can now run `resize`/`stty`
/// (or the /etc/profile helper) to report its real size, and it sticks here.
static CONSOLE_WIN_SIZE_OVERRIDE: spin::Mutex<Option<ConsoleWinSize>> = spin::Mutex::new(None);

/// Record a caller-supplied console window size (`TIOCSWINSZ`). A row/col of 0
/// clears the override and falls back to the framebuffer-derived size.
pub fn set_console_win_size(ws: ConsoleWinSize) {
    let mut ov = CONSOLE_WIN_SIZE_OVERRIDE.lock();
    if ws.ws_row == 0 && ws.ws_col == 0 {
        *ov = None;
    } else {
        *ov = Some(ws);
    }
}

/// Returns the size information of the console, see [`ConsoleWinSize`].
pub fn console_win_size() -> ConsoleWinSize {
    if let Some(ws) = *CONSOLE_WIN_SIZE_OVERRIDE.lock() {
        return ws;
    }
    #[cfg(feature = "graphic")]
    if let Some(&winsz) = CONSOLE_WIN_SIZE.try_get() {
        return winsz;
    }
    // Sensible serial default when no graphic console and no `TIOCSWINSZ`
    // override yet. Returning 0×0 makes ncurses/busybox assume 80×24 anyway,
    // but an explicit size keeps `stty size` and apps consistent.
    ConsoleWinSize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

#[macro_export]
macro_rules! klog_info {
    ($($arg:tt)*) => {
        $crate::console::klog_emit(
            $crate::console::LOG_INFO,
            &::alloc::format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! klog_warn {
    ($($arg:tt)*) => {
        $crate::console::klog_emit(
            $crate::console::LOG_WARNING,
            &::alloc::format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! klog_err {
    ($($arg:tt)*) => {
        $crate::console::klog_emit(
            $crate::console::LOG_ERR,
            &::alloc::format!($($arg)*),
        )
    };
}

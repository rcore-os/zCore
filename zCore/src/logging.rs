use core::fmt::{self, Write};
use log::{self, Level, LevelFilter, Log, Metadata, Record};

// ---------------------------------------------------------------------------
// Kernel log ring buffer  (exposed as "dmesg")
// ---------------------------------------------------------------------------
//
// A fixed circular buffer holds all kernel log messages. Access is serialized
// with `lock::Mutex` (IRQ-safe ticket spinlock) so it is safe to call from any
// context, including interrupt handlers.
//
// Sized to hold a full X-server startup syscall trace (tens of thousands of
// lines) so the structurally interesting calls are not evicted before `dmesg`
// can read them back.
const KLOG_BUF_SIZE: usize = 8 * 1024 * 1024; // 8 MiB

struct KlogBuf {
    buf: [u8; KLOG_BUF_SIZE],
    head: usize, // write pointer (wraps around)
    used: usize, // bytes currently stored (≤ KLOG_BUF_SIZE)
}

impl KlogBuf {
    const fn new() -> Self {
        Self {
            buf: [0u8; KLOG_BUF_SIZE],
            head: 0,
            used: 0,
        }
    }

    /// Append bytes; oldest data is silently overwritten when full.
    fn write(&mut self, data: &[u8]) {
        for &b in data {
            self.buf[self.head] = b;
            self.head = (self.head + 1) % KLOG_BUF_SIZE;
            if self.used < KLOG_BUF_SIZE {
                self.used += 1;
            }
        }
    }

    /// Copy the stored bytes (oldest first) into `dst`.
    /// Returns the number of bytes written.
    fn read_all(&self, dst: &mut [u8]) -> usize {
        let len = self.used.min(dst.len());
        if len == 0 {
            return 0;
        }
        // start = position of oldest byte
        let start = if self.used < KLOG_BUF_SIZE {
            0
        } else {
            self.head // head points to the oldest byte when full
        };
        for (i, d) in dst[..len].iter_mut().enumerate() {
            *d = self.buf[(start + i) % KLOG_BUF_SIZE];
        }
        len
    }

    fn size(&self) -> usize {
        self.used
    }
}

// The dmesg ring lock is a `lock::Mutex` (IRQ-disabling ticket lock), NOT a
// raw CAS spinlock. This is load-bearing — the previous hand-rolled AtomicBool
// lock froze the whole machine: it did not mask interrupts, and EVERY log line
// takes this lock (the ring copy in `SimpleLogger::log` and every `klog_*!`).
// If a timer IRQ landed while its CPU was inside the critical section and the
// handler itself logged anything (thermal-governor transition, an xhci/apic
// warn, frametrack), the handler re-entered the same lock ON THE SAME CPU and
// spun forever with IRQs off — no panic, no deadlock report (the raw loop was
// invisible to the spin-diagnostics), console dead mid-line, every other CPU
// wedging at its own next log line. Observed on real 16-thread hardware,
// reproducibly, ~6-7s into fork()'s CPU-pegged eager copy (right when the
// thermal governor logs its first throttle transition). `lock::Mutex` masks
// IRQs for the (short) critical section, making the re-entry impossible, and
// participates in the >8s-spin deadlock self-report.
static KLOG: lock::Mutex<KlogBuf> = lock::Mutex::new(KlogBuf::new());

/// Write a slice of bytes into the kernel log ring buffer.
fn klog_write(data: &[u8]) {
    KLOG.lock().write(data);
}

/// Copy the full kernel log into `dst` (oldest first).
/// Returns the number of bytes written.
pub fn klog_read_all(dst: &mut [u8]) -> usize {
    KLOG.lock().read_all(dst)
}

/// Total bytes currently stored in the kernel log ring buffer.
pub fn klog_size() -> usize {
    KLOG.lock().size()
}

/// Write a kernel message into the dmesg ring buffer only (not echoed to the graphic/serial console).
/// `priority` follows syslog(3): 3=err, 4=warn, 6=info, 7=debug.
pub fn klog_emit(priority: u8, msg: &str) {
    let now = kernel_hal::timer::timer_now();
    let micros = now.as_micros();
    let mut line = [0u8; 512];
    struct W<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }
    impl fmt::Write for W<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let n = s.len().min(self.buf.len().saturating_sub(self.pos));
            self.buf[self.pos..self.pos + n].copy_from_slice(&s.as_bytes()[..n]);
            self.pos += n;
            Ok(())
        }
    }
    let pos = {
        let mut w = W {
            buf: &mut line,
            pos: 0,
        };
        let _ = writeln!(
            w,
            "<{prio}>[{s:>3}.{us:06}] {msg}",
            prio = priority,
            s = micros / 1_000_000,
            us = micros % 1_000_000,
            msg = msg.trim_end_matches('\n'),
        );
        w.pos
    };
    klog_write(&line[..pos]);
}

/// Initialize logging with the default max log level (WARN).
pub fn init() {
    static LOGGER: SimpleLogger = SimpleLogger;
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(LevelFilter::Warn);
    // Register the ring-buffer accessors so linux-syscall can read them.
    kernel_hal::console::klog_register(klog_read_all, klog_size, klog_emit);
}

/// Reset max log level.
pub fn set_max_level(level: &str) {
    log::set_max_level(level.parse().unwrap_or(LevelFilter::Warn));
}

/// Run `f` with ALL `log`-crate output suppressed (level = Off), restoring the
/// previous max level afterwards. Used to bring the GPU up quietly at boot: the
/// RM / bring-up narration -- much of it deliberately at ERROR level so it is
/// visible during step debugging -- would otherwise flood the desktop console
/// with ugly, alarming-looking lines even though the bring-up is routine now.
/// Nothing is lost: the per-line detail is still captured into the
/// `/proc/gpustep*` buffers. Note `klog_emit` (and thus `klog_info!`) writes
/// straight to the ring buffer and is NOT gated by the level, so a clean
/// summary line emitted via `klog_info!` still prints while this is in effect.
// Used by the Linux userboot path; the zircon build never calls it.
#[allow(dead_code)]
pub fn with_output_suppressed<F: FnOnce()>(f: F) {
    let prev = log::max_level();
    log::set_max_level(LevelFilter::Off);
    f();
    log::set_max_level(prev);
}

#[macro_export]
macro_rules! klog_info {
    ($($arg:tt)*) => {
        $crate::logging::klog_emit(6, &::alloc::format!($($arg)*))
    };
}

#[macro_export]
macro_rules! klog_warn {
    ($($arg:tt)*) => {
        $crate::logging::klog_emit(4, &::alloc::format!($($arg)*))
    };
}

#[macro_export]
macro_rules! klog_err {
    ($($arg:tt)*) => {
        $crate::logging::klog_emit(3, &::alloc::format!($($arg)*))
    };
}

#[inline]
pub fn print(args: fmt::Arguments) {
    kernel_hal::console::console_write_fmt(args);
}

#[allow(dead_code)]
#[inline]
pub fn debug_print(args: fmt::Arguments) {
    kernel_hal::console::debug_write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::logging::print(core::format_args!($($arg)*));
    }
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\r\n"));
    ($($arg:tt)*) => {
        $crate::logging::print(core::format_args!($($arg)*));
        $crate::print!("\r\n");
    }
}

#[macro_export]
macro_rules! debug_print {
    ($($arg:tt)*) => {
        $crate::logging::debug_print(core::format_args!($($arg)*));
    }
}

#[macro_export]
macro_rules! debug_println {
    () => ($crate::print!("\r\n"));
    ($($arg:tt)*) => {
        $crate::logging::debug_print(core::format_args!($($arg)*));
        $crate::debug_print!("\r\n");
    }
}

#[allow(dead_code)]
#[repr(u8)]
enum ColorCode {
    Black = 30,
    Red = 31,
    Green = 32,
    Yellow = 33,
    Blue = 34,
    Magenta = 35,
    Cyan = 36,
    White = 37,
    BrightBlack = 90,
    BrightRed = 91,
    BrightGreen = 92,
    BrightYellow = 93,
    BrightBlue = 94,
    BrightMagenta = 95,
    BrightCyan = 96,
    BrightWhite = 97,
}

/// Add escape sequence to print with color in Linux console
macro_rules! with_color {
    ($color_code:expr, $($arg:tt)*) => {{
        #[cfg(feature = "colorless-log")]
        { let _ = $color_code; format_args!($($arg)*) }
        #[cfg(not(feature = "colorless-log"))]
        { format_args!("\u{1B}[{}m{}\u{1B}[m", $color_code as u8, format_args!($($arg)*)) }
    }};
}

struct SimpleLogger;

impl Log for SimpleLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // After a heap/stack smash, a log Record's args can point at freed or
        // scribbled memory; formatting them is what re-faulted in a STORM right
        // here — a READ #PF inside `SimpleLogger::log` on a mangled arg pointer
        // (`rip` symbolizes to this function) — that buried the crash and could
        // keep the machine from surviving long enough to catch the writer. Once
        // a smash is suspected, stop touching Record args entirely. The direct
        // serial diagnostics (`[null-exec]`, `[watchpoint]`, `oops`) do NOT
        // route through `log::`, so they still print in full.
        if executor::heap_smash_suspected() {
            use core::sync::atomic::{AtomicBool, Ordering};
            static NOTED: AtomicBool = AtomicBool::new(false);
            if !NOTED.swap(true, Ordering::Relaxed) {
                kernel_hal::console::serial_write_str(
                    "[logging] heap smash suspected — suppressing log:: formatting \
                     from here (args may be corrupt); direct-serial diagnostics \
                     still print\r\n",
                );
            }
            return;
        }
        let now = kernel_hal::timer::timer_now();
        let cpu_id = kernel_hal::cpu::cpu_id();
        let (tid, pid) = (0, 0); //kernel_hal::thread::get_tid();
        let level = record.level();
        let target = record.target();
        let level_color = match level {
            Level::Error => ColorCode::BrightRed,
            Level::Warn => ColorCode::BrightYellow,
            Level::Info => ColorCode::BrightGreen,
            Level::Debug => ColorCode::BrightCyan,
            Level::Trace => ColorCode::BrightBlack,
        };
        let args_color = match level {
            Level::Error => ColorCode::Red,
            Level::Warn => ColorCode::Yellow,
            Level::Info => ColorCode::Green,
            Level::Debug => ColorCode::Cyan,
            Level::Trace => ColorCode::BrightBlack,
        };
        // Primary log output: serial + (later) native graphic console.
        print(with_color!(
            ColorCode::White,
            "[{time} {level} {info} {data}\n",
            time = {
                cfg_if! {
                    if #[cfg(feature = "libos")] {
                        use chrono::{TimeZone, Local};
                        Local.timestamp_nanos(now.as_nanos() as _).format("%Y-%m-%d %H:%M:%S%.6f")
                    } else {
                        let micros = now.as_micros();
                        format_args!("{s:>3}.{us:06}", s = micros / 1_000_000, us = micros % 1_000_000)
                    }
                }
            },
            level = with_color!(level_color, "{level:<5}"),
            info = with_color!(ColorCode::White, "{cpu_id} {pid}:{tid} {target}]"),
            data = with_color!(args_color, "{args}", args = record.args()),
        ));

        // Also write a plain-text copy into the ring buffer for dmesg.
        {
            struct KlogWriter {
                buf: [u8; 1024],
                pos: usize,
            }
            impl fmt::Write for KlogWriter {
                fn write_str(&mut self, s: &str) -> fmt::Result {
                    let bytes = s.as_bytes();
                    let free = self.buf.len().saturating_sub(self.pos);
                    let n = bytes.len().min(free);
                    self.buf[self.pos..self.pos + n].copy_from_slice(&bytes[..n]);
                    self.pos += n;
                    Ok(())
                }
            }
            let mut w = KlogWriter {
                buf: [0u8; 1024],
                pos: 0,
            };
            let micros = now.as_micros();
            let syslog_prio = match level {
                Level::Error => 3u8,
                Level::Warn => 4,
                Level::Info => 6,
                Level::Debug => 7,
                Level::Trace => 7,
            };
            let _ = core::fmt::write(
                &mut w,
                format_args!(
                    "<{prio}>[{s:>3}.{us:06}] {args}\n",
                    prio = syslog_prio,
                    s = micros / 1_000_000,
                    us = micros % 1_000_000,
                    args = record.args(),
                ),
            );
            klog_write(&w.buf[..w.pos]);
        }

        // When running with `LOG=debug` (or more verbose) we still don't have a native GPU
        // driver early in boot. Mirror logs to the UEFI GOP framebuffer console so we can
        // see early boot progress on real hardware.
        //
        // IMPORTANT: The early framebuffer console can't interpret ANSI escapes, so
        // keep this output plain (no colors).
        #[cfg(feature = "graphic")]
        if log::max_level() >= LevelFilter::Debug {
            cfg_if! {
                if #[cfg(feature = "libos")] {
                    use chrono::{TimeZone, Local};
                    kernel_hal::console::debug_write_fmt(format_args!(
                        "[{time} {level:<5} {cpu_id} {pid}:{tid} {target}] {args}\n",
                        time = Local.timestamp_nanos(now.as_nanos() as _).format("%Y-%m-%d %H:%M:%S%.6f"),
                        level = level,
                        cpu_id = cpu_id,
                        pid = pid,
                        tid = tid,
                        target = target,
                        args = record.args(),
                    ));
                } else {
                    let micros = now.as_micros();
                    let s = micros / 1_000_000;
                    let us = micros % 1_000_000;
                    kernel_hal::console::debug_write_fmt(format_args!(
                        "[{s:>3}.{us:06} {level:<5} {cpu_id} {pid}:{tid} {target}] {args}\n",
                        s = s,
                        us = us,
                        level = level,
                        cpu_id = cpu_id,
                        pid = pid,
                        tid = tid,
                        target = target,
                        args = record.args(),
                    ));
                }
            }
        }
    }

    fn flush(&self) {}
}

//! Kernel log (dmesg) helpers for drivers.
//!
//! These bypass the `log` crate level filter so vital events (link up/down, etc.)
//! are always visible via `dmesg` / `syslog(2)`.

use core::fmt::{self, Write};

extern "C" {
    fn drivers_klog_emit(priority: u8, msg: *const u8, len: usize);
}

/// Syslog priority (same as Linux `syslog.h`).
pub const LOG_ERR: u8 = 3;
pub const LOG_WARNING: u8 = 4;
pub const LOG_INFO: u8 = 6;

fn emit(priority: u8, msg: &str) {
    if msg.is_empty() {
        return;
    }
    unsafe { drivers_klog_emit(priority, msg.as_ptr(), msg.len()) };
}

/// Format and emit one line to the kernel log (dmesg).
pub fn klog_emit(priority: u8, args: fmt::Arguments<'_>) {
    let mut buf = [0u8; 256];
    let mut pos = {
        let mut w = KlogBufWriter {
            buf: &mut buf,
            pos: 0,
        };
        let _ = w.write_fmt(args);
        w.pos
    };
    if pos < buf.len() && (pos == 0 || buf[pos - 1] != b'\n') {
        buf[pos] = b'\n';
        pos += 1;
    }
    emit(priority, core::str::from_utf8(&buf[..pos]).unwrap_or(""));
}

struct KlogBufWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl Write for KlogBufWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(self.buf.len().saturating_sub(self.pos));
        self.buf[self.pos..self.pos + n].copy_from_slice(&s.as_bytes()[..n]);
        self.pos += n;
        Ok(())
    }
}

#[macro_export]
macro_rules! klog_info {
    ($($arg:tt)*) => {
        $crate::bus::klog::klog_emit(
            $crate::bus::klog::LOG_INFO,
            core::format_args!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! klog_warn {
    ($($arg:tt)*) => {
        $crate::bus::klog::klog_emit(
            $crate::bus::klog::LOG_WARNING,
            core::format_args!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! klog_err {
    ($($arg:tt)*) => {
        $crate::bus::klog::klog_emit(
            $crate::bus::klog::LOG_ERR,
            core::format_args!($($arg)*),
        )
    };
}

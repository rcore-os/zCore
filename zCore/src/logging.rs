use core::fmt;
#[cfg(not(feature = "colorless-log"))]
use log::Level;
use log::{self, LevelFilter, Log, Metadata, Record};

/// Initialize logging with the default max log level (WARN).
pub fn init() {
    static LOGGER: SimpleLogger = SimpleLogger;
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(LevelFilter::Warn);
}

/// Reset max log level.
pub fn set_max_level(level: &str) {
    log::set_max_level(level.parse().unwrap_or(LevelFilter::Warn));
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

struct SimpleLogger;

impl Log for SimpleLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let now = kernel_hal::timer::timer_now();
        let cpu_id = kernel_hal::cpu::cpu_id();
        let (tid, pid) = (0, 0); //kernel_hal::thread::get_tid();
        let level = record.level();
        let target = record.target();
        #[cfg(not(feature = "colorless-log"))]
        let level_color = match level {
            Level::Error => ColorCode::BrightRed,
            Level::Warn => ColorCode::BrightYellow,
            Level::Info => ColorCode::BrightGreen,
            Level::Debug => ColorCode::BrightCyan,
            Level::Trace => ColorCode::BrightBlack,
        };
        #[cfg(not(feature = "colorless-log"))]
        let args_color = match level {
            Level::Error => ColorCode::Red,
            Level::Warn => ColorCode::Yellow,
            Level::Info => ColorCode::Green,
            Level::Debug => ColorCode::Cyan,
            Level::Trace => ColorCode::BrightBlack,
        };
        let time = {
            cfg_if! {
                if #[cfg(feature = "libos")] {
                    use chrono::{Local, TimeZone};
                    alloc::format!("{}", Local.timestamp_nanos(now.as_nanos() as _).format("%Y-%m-%d %H:%M:%S%.6f"))
                } else {
                    let micros = now.as_micros();
                    alloc::format!("{s:>3}.{us:06}", s = micros / 1_000_000, us = micros % 1_000_000)
                }
            }
        };
        #[cfg(feature = "colorless-log")]
        print(format_args!(
            "[{time} {level:<5} {cpu_id} {pid}:{tid} {target}] {}\n",
            record.args()
        ));
        #[cfg(not(feature = "colorless-log"))]
        print(format_args!(
            "\u{1b}[{}m[{time} \u{1b}[{}m{level:<5}\u{1b}[m \u{1b}[{}m{cpu_id} {pid}:{tid} {target}]\u{1b}[m \u{1b}[{}m{}\u{1b}[m\n",
            ColorCode::White as u8,
            level_color as u8,
            ColorCode::White as u8,
            args_color as u8,
            record.args(),
        ));
    }

    fn flush(&self) {}
}

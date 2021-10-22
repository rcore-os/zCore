use core::fmt;
use log::{self, Level, LevelFilter, Log, Metadata, Record};

pub fn init() {
    static LOGGER: SimpleLogger = SimpleLogger;
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(LevelFilter::Warn);
}

pub fn set_max_level(level: &str) {
    log::set_max_level(level.parse().unwrap_or(LevelFilter::Off));
}

pub fn print(args: fmt::Arguments) {
    kernel_hal::console::console_write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        $crate::logging::print(format_args!($($arg)*));
    });
}

#[macro_export]
macro_rules! println {
    ($fmt:expr) => (print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => (print!(concat!($fmt, "\n"), $($arg)*));
}

#[repr(u8)]
#[allow(dead_code)]
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
        #[cfg(feature = "color_log")]
        { format_args!("\u{1B}[{}m{}\u{1B}[m", $color_code as u8, format_args!($($arg)*)) }
        #[cfg(not(feature = "color_log"))]
        { format_args!($($arg)*) }
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

        let now = kernel_hal::timer::timer_now();
        let cpu_id = kernel_hal::cpu::cpu_id();
        let (tid, pid) = kernel_hal::thread::get_tid();
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
        print(with_color!(
            ColorCode::White,
            "[{} {} {} {}\n",
            {
                cfg_if! {
                    if #[cfg(feature = "libos")] {
                        use chrono::{TimeZone, Local};
                        Local.timestamp_nanos(now.as_nanos() as _).format("%Y-%m-%d %H:%M:%S%.6f")
                    } else {
                        let micros = now.as_micros();
                        format_args!("{:>3}.{:06}", micros / 1_000_000, micros % 1_000_000)
                    }
                }
            },
            with_color!(level_color, "{:<5}", level),
            with_color!(ColorCode::White, "{} {}:{} {}]", cpu_id, pid, tid, target),
            with_color!(args_color, "{}", record.args()),
        ));
    }

    fn flush(&self) {}
}

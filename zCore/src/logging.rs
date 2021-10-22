use {
    core::fmt,
    log::{self, Level, LevelFilter, Log, Metadata, Record},
};

pub fn init() {
    static LOGGER: SimpleLogger = SimpleLogger;
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(LevelFilter::Warn);
}

pub fn set_max_level(level: &str) {
    log::set_max_level(level.parse().unwrap_or(LevelFilter::Off));
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

/// Add escape sequence to print with color in Linux console
macro_rules! with_color {
    ($args: ident, $color_code: ident) => {{
        format_args!("\u{1B}[{}m{}\u{1B}[0m", $color_code as u8, $args)
    }};
}

fn print_in_color(args: fmt::Arguments, color_code: u8) {
    kernel_hal::console::console_write_fmt(with_color!(args, color_code));
}

#[allow(dead_code)]
pub fn print(args: fmt::Arguments) {
    kernel_hal::console::console_write_fmt(args);
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

        let (tid, pid) = (0, 0); // kernel_hal::thread::get_tid(); // FIXME
        print_in_color(
            format_args!(
                "[{:?} {:>5} {} {}:{}] {}\n",
                kernel_hal::timer::timer_now(),
                record.level(),
                kernel_hal::cpu::cpu_id(),
                pid,
                tid,
                record.args()
            ),
            level_to_color_code(record.level()),
        );
    }
    fn flush(&self) {}
}

fn level_to_color_code(level: Level) -> u8 {
    match level {
        Level::Error => 31, // Red
        Level::Warn => 93,  // BrightYellow
        Level::Info => 34,  // Blue
        Level::Debug => 32, // Green
        Level::Trace => 90, // BrightBlack
    }
}

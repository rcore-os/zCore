use core::fmt;
use log::{LevelFilter, Log, Metadata, Record};

#[cfg(feature = "libos")]
static FLUSH_EACH_RECORD: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Initialize kernel logging independently of the user console.
pub fn init() {
    #[cfg(feature = "libos")]
    let _ = kernel_log_file();
    static LOGGER: SimpleLogger = SimpleLogger;
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(LevelFilter::Info);
}

pub fn set_max_level(level: &str) {
    log::set_max_level(level.parse().unwrap_or(LevelFilter::Info));
}

#[cfg(feature = "libos")]
fn kernel_log_file() -> &'static std::sync::Mutex<std::io::BufWriter<std::fs::File>> {
    static FILE: std::sync::OnceLock<std::sync::Mutex<std::io::BufWriter<std::fs::File>>> =
        std::sync::OnceLock::new();
    FILE.get_or_init(|| {
        let path = std::env::var_os("ZCORE_KERNEL_LOG").unwrap_or_else(|| "kernel.log".into());
        std::sync::Mutex::new(std::io::BufWriter::with_capacity(
            64 * 1024,
            std::fs::File::create(path).expect("create kernel log"),
        ))
    })
}

#[inline]
pub fn print(args: fmt::Arguments) {
    #[cfg(feature = "libos")]
    {
        use std::io::Write;
        let mut file = kernel_log_file().lock().unwrap();
        let _ = file.write_fmt(args);
        if FLUSH_EACH_RECORD.load(core::sync::atomic::Ordering::Relaxed) {
            let _ = file.flush();
        }
    }
    #[cfg(not(feature = "libos"))]
    kernel_hal::console::debug_write_fmt(args);
}

#[allow(dead_code)]
#[inline]
pub fn debug_print(args: fmt::Arguments) {
    print(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::logging::print(core::format_args!($($arg)*));
    }
}

#[macro_export]
macro_rules! println {
    () => ($crate::logging::print(core::format_args!("\n")));
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
    () => ($crate::logging::print(core::format_args!("\n")));
    ($($arg:tt)*) => {
        $crate::logging::debug_print(core::format_args!($($arg)*));
        $crate::debug_print!("\r\n");
    }
}

struct SimpleLogger;

impl Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let micros = kernel_hal::timer::timer_now().as_micros();
        print(format_args!(
            "[{s:>3}.{us:06} {level:<5} cpu={cpu} {target}] {message}\n",
            s = micros / 1_000_000,
            us = micros % 1_000_000,
            level = record.level(),
            cpu = kernel_hal::cpu::cpu_id(),
            target = record.target(),
            message = record.args(),
        ));
        if record.level() == log::Level::Error {
            self.flush();
        }
    }

    fn flush(&self) {
        #[cfg(feature = "libos")]
        {
            use std::io::Write;
            let _ = kernel_log_file().lock().unwrap().flush();
        }
    }
}

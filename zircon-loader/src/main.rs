#![deny(warnings, unused_must_use)]

extern crate log;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use structopt::StructOpt;
use zircon_loader::*;
use zircon_object::object::*;

#[derive(Debug, StructOpt)]
#[structopt()]
struct Opt {
    #[structopt(parse(from_os_str))]
    prebuilt_path: PathBuf,

    #[structopt(default_value = "")]
    cmdline: String,

    #[structopt(short, long)]
    debug: bool,
}

#[async_std::main]
async fn main() {
    kernel_hal::init();
    init_logger();

    let opt = Opt::from_args();
    let images = open_images(&opt.prebuilt_path, opt.debug).expect("failed to read file");
    let proc: Arc<dyn KernelObject> = run_userboot(&images, &opt.cmdline);
    drop(images);

    proc.wait_signal(Signal::USER_SIGNAL_0).await;
}

fn open_images(path: &Path, debug: bool) -> std::io::Result<Images<Vec<u8>>> {
    Ok(Images {
        userboot: std::fs::read(path.join("userboot-libos.so"))?,
        vdso: std::fs::read(path.join("libzircon-libos.so"))?,
        zbi: if debug {
            std::fs::read(path.join("core-tests.zbi"))?
        } else {
            std::fs::read(path.join("bringup.zbi"))?
        },
    })
}

fn init_logger() {
    env_logger::builder()
        .format(|buf, record| {
            use env_logger::fmt::Color;
            use log::Level;
            use std::io::Write;

            let (tid, pid) = kernel_hal::thread::get_tid();
            let mut style = buf.style();
            match record.level() {
                Level::Trace => style.set_color(Color::Black).set_intense(true),
                Level::Debug => style.set_color(Color::White),
                Level::Info => style.set_color(Color::Green),
                Level::Warn => style.set_color(Color::Yellow),
                Level::Error => style.set_color(Color::Red).set_bold(true),
            };
            let now = kernel_hal::timer::timer_now();
            let level = style.value(record.level());
            let args = record.args();
            writeln!(buf, "[{:?} {:>5} {}:{}] {}", now, level, pid, tid, args)
        })
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn userboot() {
        kernel_hal::init();

        let opt = Opt {
            #[cfg(target_arch = "x86_64")]
            prebuilt_path: PathBuf::from("../prebuilt/zircon/x64"),
            #[cfg(target_arch = "aarch64")]
            prebuilt_path: PathBuf::from("../prebuilt/zircon/arm64"),
            cmdline: String::from(""),
            debug: false,
        };
        let images = open_images(&opt.prebuilt_path, opt.debug).expect("failed to read file");

        let proc: Arc<dyn KernelObject> = run_userboot(&images, &opt.cmdline);
        drop(images);

        proc.wait_signal(Signal::PROCESS_TERMINATED).await;
    }
}

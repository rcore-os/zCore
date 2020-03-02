#![deny(warnings, unused_must_use)]

extern crate log;

use std::sync::Arc;
use {std::path::PathBuf, structopt::StructOpt, zircon_loader::*, zircon_object::object::*};

#[derive(Debug, StructOpt)]
#[structopt()]
struct Opt {
    #[structopt(parse(from_os_str))]
    prebuilt_path: PathBuf,

    #[structopt(default_value = "")]
    cmdline: String,
}

#[async_std::main]
async fn main() {
    kernel_hal_unix::init();
    env_logger::init();

    let opt = Opt::from_args();
    let images = open_images(&opt.prebuilt_path).expect("failed to read file");

    let proc: Arc<dyn KernelObject> = run_userboot(&images, &opt.cmdline);
    drop(images);

    proc.wait_signal_async(Signal::PROCESS_TERMINATED).await;
}

fn open_images(path: &PathBuf) -> std::io::Result<Images<Vec<u8>>> {
    Ok(Images {
        userboot: std::fs::read(path.join("userboot-libos.so"))?,
        vdso: std::fs::read(path.join("libzircon-libos.so"))?,
        decompressor: std::fs::read(path.join("decompress-zstd.so"))?,
        zbi: std::fs::read(path.join("fuchsia.zbi"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn userboot() {
        kernel_hal_unix::init();

        let opt = Opt {
            prebuilt_path: PathBuf::from("../prebuilt/zircon"),
            cmdline: String::from(""),
        };
        let images = open_images(&opt.prebuilt_path).expect("failed to read file");

        let proc: Arc<dyn KernelObject> = run_userboot(&images, &opt.cmdline);
        drop(images);

        proc.wait_signal_async(Signal::PROCESS_TERMINATED).await;
    }
}

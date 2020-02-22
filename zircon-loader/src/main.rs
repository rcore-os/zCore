#![deny(warnings, unused_must_use)]

extern crate log;

use std::sync::Arc;
use {std::path::PathBuf, structopt::StructOpt, zircon_loader::*, zircon_object::object::*};

#[derive(Debug, StructOpt)]
#[structopt()]
struct Opt {
    #[structopt(parse(from_os_str))]
    userboot_path: PathBuf,

    #[structopt(parse(from_os_str))]
    vdso_path: PathBuf,

    #[structopt(parse(from_os_str))]
    zbi_path: PathBuf,

    #[structopt(parse(from_os_str))]
    decompressor_path: PathBuf,

    #[structopt(default_value = "")]
    cmdline: String,
}

#[async_std::main]
async fn main() {
    kernel_hal_unix::init();
    env_logger::init();

    let opt = Opt::from_args();
    let userboot_data = std::fs::read(opt.userboot_path).expect("failed to read file");
    let vdso_data = std::fs::read(opt.vdso_path).expect("failed to read file");
    let zbi_data = std::fs::read(opt.zbi_path).expect("failed to read file");
    let decompressor_data = std::fs::read(opt.decompressor_path).expect("failed to read file");

    let proc: Arc<dyn KernelObject> = run_userboot(
        &userboot_data,
        &vdso_data,
        &decompressor_data,
        &zbi_data,
        &opt.cmdline,
    );
    proc.wait_signal_async(Signal::PROCESS_TERMINATED).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn userboot() {
        kernel_hal_unix::init();

        let base = PathBuf::from("../prebuilt");
        let opt = Opt {
            userboot_path: base.join("userboot.so"),
            vdso_path: base.join("libzircon.so"),
            zbi_path: base.join("legacy-image-x64.zbi"),
            cmdline: String::from(""),
        };
        let userboot_data = std::fs::read(opt.userboot_path).expect("failed to read file");
        let vdso_data = std::fs::read(opt.vdso_path).expect("failed to read file");
        let zbi_data = std::fs::read(opt.zbi_path).expect("failed to read file");

        let proc: Arc<dyn KernelObject> =
            run_userboot(&userboot_data, &vdso_data, &zbi_data, &opt.cmdline);
        proc.wait_signal_async(Signal::PROCESS_TERMINATED).await;
    }
}

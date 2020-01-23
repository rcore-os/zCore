#![deny(warnings, unused_must_use)]

extern crate log;

use {linux_loader::*, std::path::PathBuf, structopt::StructOpt, zircon_object::object::*};

#[derive(Debug, StructOpt)]
#[structopt()]
struct Opt {
    #[structopt(parse(from_os_str))]
    libc_path: PathBuf,
}

fn main() {
    zircon_hal_unix::init();
    env_logger::init();

    let opt = Opt::from_args();
    let libc_data = std::fs::read(opt.libc_path).expect("failed to read file");

    let args = vec![String::from("./prebuilt/busybox")]; // TODO
    let envs = vec![]; // TODO
    let proc = run(&libc_data, args, envs);
    proc.wait_signal(Signal::PROCESS_TERMINATED);
}

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
    env_logger::init();
    zircon_hal_unix::init();

    let opt = Opt::from_args();
    let libc_data = std::fs::read(opt.libc_path).expect("failed to read file");

    let args = vec![String::from("host/busybox")]; // TODO
    let envs = vec![]; // TODO
    let proc = run(&libc_data, args, envs);
    proc.wait_signal(Signal::PROCESS_TERMINATED);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_libc() {
        zircon_hal_unix::init();

        let base = PathBuf::from("../prebuilt");
        let opt = Opt {
            libc_path: base.join("libc.so"),
        };
        let libc_data = std::fs::read(opt.libc_path).expect("failed to read file");

        let args = vec![]; // TODO
        let envs = vec![]; // TODO
        let proc = run(&libc_data, args, envs);
        proc.wait_signal(Signal::PROCESS_TERMINATED);
    }
}

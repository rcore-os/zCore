#![deny(warnings, unused_must_use)]

extern crate log;

use linux_loader::*;
use rcore_fs_hostfs::HostFS;
use zircon_object::object::*;

fn main() {
    env_logger::init();
    kernel_hal_unix::init();

    let args: Vec<_> = std::env::args().skip(1).collect();
    let envs = vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin:/usr/x86_64-alpine-linux-musl/bin".into()];

    let exec_path = args[0].clone();
    let hostfs = HostFS::new("rootfs");
    let proc = run(&exec_path, args, envs, hostfs);
    proc.wait_signal(Signal::PROCESS_TERMINATED);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_libc() {
        kernel_hal_unix::init();

        let args = vec![String::from("/bin/busybox")];
        let envs = vec![]; // TODO

        let exec_path = args[0].clone();
        let hostfs = HostFS::new("../rootfs");
        let proc = run(&exec_path, args, envs, hostfs);
        proc.wait_signal(Signal::PROCESS_TERMINATED);
    }
}

#![deny(warnings, unused_must_use)]

extern crate log;

use linux_loader::*;
use linux_syscall::ProcessExt;
use rcore_fs_hostfs::HostFS;
use zircon_object::object::*;

fn main() {
    env_logger::init();
    kernel_hal_unix::init();

    let args: Vec<_> = std::env::args().skip(1).collect();
    let envs: Vec<_> = std::env::vars()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();

    let libc_path = &args[0];
    let libc_data = std::fs::read(libc_path).expect("failed to read file");

    let proc = run(&libc_data, args, envs);

    // file system
    let hostfs = HostFS::new("prebuilt");
    proc.lock_linux().mount("host", hostfs);

    proc.wait_signal(Signal::PROCESS_TERMINATED);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_libc() {
        kernel_hal_unix::init();

        let libc_data = std::fs::read("../prebuilt/libc.so").expect("failed to read file");

        let args = vec!["libc.so".into(), "host/busybox".into()];
        let envs = vec![]; // TODO
        let proc = run(&libc_data, args, envs);

        // file system
        let hostfs = HostFS::new("../prebuilt");
        proc.lock_linux().mount("host", hostfs);

        proc.wait_signal(Signal::PROCESS_TERMINATED);
    }
}

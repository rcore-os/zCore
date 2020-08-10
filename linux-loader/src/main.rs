//! Linux LibOS entrance
#![deny(warnings, unused_must_use, missing_docs)]
#![feature(thread_id_value)]

extern crate log;

use linux_loader::*;
use rcore_fs_hostfs::HostFS;
use std::io::Write;
use std::sync::Arc;
use zircon_object::object::*;

/// main entry
#[async_std::main]
async fn main() {
    // init loggger for debug
    init_logger();
    // init HAL implementation on unix
    kernel_hal_unix::init();
    // run first process
    let args: Vec<_> = std::env::args().skip(1).collect();
    let envs = vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin:/usr/x86_64-alpine-linux-musl/bin".into()];

    let hostfs = HostFS::new("rootfs");
    let proc: Arc<dyn KernelObject> = run(args, envs, hostfs);
    proc.wait_signal(Signal::PROCESS_TERMINATED).await;
}

/// init the env_logger
fn init_logger() {
    env_logger::builder()
        .format(|buf, record| {
            use env_logger::fmt::Color;
            use log::Level;

            let tid = async_std::task::current().id();
            let mut style = buf.style();
            match record.level() {
                Level::Trace => style.set_color(Color::Black).set_intense(true),
                Level::Debug => style.set_color(Color::White),
                Level::Info => style.set_color(Color::Green),
                Level::Warn => style.set_color(Color::Yellow),
                Level::Error => style.set_color(Color::Red).set_bold(true),
            };
            let level = style.value(record.level());
            writeln!(buf, "[{:>5}][{}] {}", level, tid, record.args())
        })
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io};

    /// test with cmd line
    async fn test(cmdline: &str) {
        kernel_hal_unix::init();

        let args: Vec<String> = cmdline.split(' ').map(|s| s.into()).collect();
        let envs =
            vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin:/usr/x86_64-alpine-linux-musl/bin".into()]; // TODO
        let hostfs = HostFS::new("../rootfs");
        let proc = run(args, envs, hostfs);
        let proc: Arc<dyn KernelObject> = proc;
        proc.wait_signal(Signal::PROCESS_TERMINATED).await;
    }

    // test using busybox

    #[async_std::test]
    async fn test_busybox() {
        test("/bin/busybox").await;
    }

    #[async_std::test]
    async fn test_uname() {
        test("/bin/busybox uname -a").await;
    }

    #[async_std::test]
    async fn test_date() {
        test("/bin/busybox date").await;
    }

    #[async_std::test]
    async fn test_dir() {
        test("/bin/busybox pwd").await;
        test("/bin/busybox ls -a").await;
        test("/bin/busybox dirname /bin/busybox").await;
    }

    #[async_std::test]
    async fn test_create_remove_file() {
        fs::read("../rootfs/testfile").unwrap_err();
        test("/bin/busybox touch testfile").await;
        fs::read("../rootfs/testfile").unwrap();
        test("/bin/busybox touch testfile").await;
        fs::read("../rootfs/testfile").unwrap();
        test("/bin/busybox rm testfile").await;
        fs::read("../rootfs/testfile").unwrap_err();
    }

    #[async_std::test]
    async fn test_create_remove_dir() {
        fs::read_dir("../rootfs/test").unwrap_err();
        test("/bin/busybox mkdir test").await;
        fs::read_dir("../rootfs/test").unwrap();
        test("/bin/busybox rmdir test").await;
        fs::read_dir("../rootfs/test").unwrap_err();
    }

    #[async_std::test]
    async fn test_readfile() {
        test("/bin/busybox cat /etc/profile").await;
    }

    #[async_std::test]
    async fn test_cp_mv() {
        fs::read("../rootfs/etc/hostname.bak").unwrap_err();
        test("/bin/busybox cp /etc/hostname /etc/hostname.bak").await;
        fs::read("../rootfs/etc/hostname.bak").unwrap();
        test("/bin/busybox mv /etc/hostname.bak /etc/hostname.mv").await;
        fs::read("../rootfs/etc/hostname.bak").unwrap_err();
    }

    #[async_std::test]
    async fn test_link() {
        fs::read("../rootfs/etc/hostname.ln").unwrap_err();
        test("/bin/busybox ln /etc/hostname /etc/hostname.ln").await;
        fs::read("../rootfs/etc/hostname.ln").unwrap();
        test("/bin/busybox unlink /etc/hostname.ln").await;
        fs::read("../rootfs/etc/hostname.ln").unwrap_err();
    }

    #[async_std::test]
    async fn test_env() {
        test("/bin/busybox env").await;
    }

    // syscall unit test

    #[async_std::test]
    async fn test_pipe() {
        test("/bin/testpipe1").await;
        let str = fs::read_to_string("../rootfs/testpipe.txt").unwrap();
        io::stdout().write(str.as_bytes()).unwrap();
        test("/bin/busybox rm testpipe.txt").await;
    }

    #[async_std::test]
    async fn test_time() {
        test("/bin/testtime").await;
    }
}

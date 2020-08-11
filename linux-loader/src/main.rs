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
    use std::fs;
    use zircon_object::object::task::*;

    /// test with cmd line
    async fn test(cmdline: &str) -> i64 {
        kernel_hal_unix::init();

        let args: Vec<String> = cmdline.split(' ').map(|s| s.into()).collect();
        let envs =
            vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin:/usr/x86_64-alpine-linux-musl/bin".into()]; // TODO
        let hostfs = HostFS::new("../rootfs");
        let proc = run(args, envs, hostfs);
        let procobj: Arc<dyn KernelObject> = proc.clone();
        procobj.wait_signal(Signal::PROCESS_TERMINATED).await;
        if let Status::Exited(code) = proc.status() {
            return code;
        }
        -1
    }

    // test using busybox

    #[async_std::test]
    async fn test_busybox() {
        assert_eq!(test("/bin/busybox").await, 0);
    }

    #[async_std::test]
    async fn test_uname() {
        assert_eq!(test("/bin/busybox uname -a").await, 0);
    }

    #[async_std::test]
    async fn test_date_time() {
        assert_eq!(test("/bin/busybox date").await, 0);
        assert_eq!(test("/bin/busybox uptime").await, 0);
    }

    #[async_std::test]
    async fn test_dir() {
        assert_eq!(test("/bin/busybox pwd").await, 0);
        assert_eq!(test("/bin/busybox ls -a").await, 0);
        assert_eq!(test("/bin/busybox dirname /bin/busybox").await, 0);
    }

    #[async_std::test]
    async fn test_create_remove_file() {
        test("/bin/busybox rm testfile").await; // can't remove
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
        test("/bin/busybox rmdir test").await; // can't remove
        fs::read_dir("../rootfs/test").unwrap_err();
        test("/bin/busybox mkdir test").await;
        fs::read_dir("../rootfs/test").unwrap();
        test("/bin/busybox rmdir test").await;
        fs::read_dir("../rootfs/test").unwrap_err();
    }

    #[async_std::test]
    async fn test_readfile() {
        assert_eq!(test("/bin/busybox cat /etc/profile").await, 0);
        assert_eq!(test("/bin/busybox cat /etc/profila").await, 1); // can't open
    }

    #[async_std::test]
    async fn test_cp_mv() {
        test("/bin/busybox cp /etc/hostnama /etc/hostname.bak").await; // can't move
        fs::read("../rootfs/etc/hostname.bak").unwrap_err();
        test("/bin/busybox cp /etc/hostname /etc/hostname.bak").await;
        fs::read("../rootfs/etc/hostname.bak").unwrap();
        test("/bin/busybox mv /etc/hostname.bak /etc/hostname.mv").await;
        fs::read("../rootfs/etc/hostname.bak").unwrap_err();
    }

    #[async_std::test]
    async fn test_link() {
        test("/bin/busybox ln /etc/hostnama /etc/hostname.ln").await; // can't ln
        fs::read("../rootfs/etc/hostname.ln").unwrap_err();
        test("/bin/busybox ln /etc/hostname /etc/hostname.ln").await;
        fs::read("../rootfs/etc/hostname.ln").unwrap();
        test("/bin/busybox unlink /etc/hostname.ln").await;
        fs::read("../rootfs/etc/hostname.ln").unwrap_err();
    }

    #[async_std::test]
    async fn test_env() {
        assert_eq!(test("/bin/busybox env").await, 0);
    }

    #[async_std::test]
    async fn test_ps() {
        assert_eq!(test("/bin/busybox ps").await, 0);
    }

    // syscall unit test

    #[async_std::test]
    async fn test_pipe() {
        assert_eq!(test("/bin/testpipe1").await, 0);
    }

    #[async_std::test]
    async fn test_time() {
        assert_eq!(test("/bin/testtime").await, 0);
    }
}

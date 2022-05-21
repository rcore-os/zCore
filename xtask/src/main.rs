#![feature(path_file_prefix)]
#![feature(exit_status_error)]

#[macro_use]
extern crate clap;

use clap::Parser;
use std::{fs::read_to_string, net::Ipv4Addr};

#[cfg(target_arch = "x86_64")]
mod dump;

mod arch;
mod build;
mod command;
mod enums;
mod errors;

use arch::ArchArg;
use build::{AsmArgs, GdbArgs, QemuArgs};
use command::{Cargo, CommandExt, Ext, Git, Make};
use enums::*;
use errors::XError;

const ALPINE_WEBSITE: &str = "https://dl-cdn.alpinelinux.org/alpine/v3.12/releases";
const ALPINE_ROOTFS_VERSION: &str = "3.12.0";

/// Build or test zCore.
#[derive(Parser)]
#[clap(name = "zCore configure")]
#[clap(version, about, long_about = None)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Set git proxy.
    ///
    /// Input your proxy port to set the proxy,
    /// or leave blank to unset it.
    GitProxy(ProxyPort),
    /// Dump build config.
    #[cfg(target_arch = "x86_64")]
    Dump,

    /// First time running.
    Setup,
    /// Update rustup and cargo.
    UpdateAll,
    /// Check style
    CheckStyle,

    /// Build rootfs
    Rootfs(ArchArg),
    /// Put libc test into rootfs.
    LibcTest(ArchArg),
    /// Put other test into rootfs.
    OtherTest(ArchArg),
    /// Build image
    Image(ArchArg),

    /// Dump asm of kernel
    Asm(AsmArgs),
    /// Run zCore in qemu
    Qemu(QemuArgs),
    /// Launch GDB
    Gdb(GdbArgs),
}

#[derive(Args)]
struct ProxyPort {
    /// Proxy port.
    #[clap(long)]
    port: Option<u16>,
    /// Global config.
    #[clap(short, long)]
    global: bool,
}

fn main() {
    match Cli::parse().command {
        Commands::GitProxy(ProxyPort { port, global }) => {
            if let Some(port) = port {
                set_git_proxy(global, port);
            } else {
                unset_git_proxy(global);
            }
        }
        #[cfg(target_arch = "x86_64")]
        Commands::Dump => dump::dump_config(),
        Commands::Setup => {
            make_git_lfs();
            git_submodule_update(true);
        }
        Commands::UpdateAll => update_all(),
        Commands::CheckStyle => check_style(),
        Commands::Rootfs(arg) => arg.make_rootfs(true),
        Commands::LibcTest(arg) => arg.put_libc_test(),
        Commands::OtherTest(arg) => arg.put_other_test(),
        Commands::Image(arg) => arg.image(),
        Commands::Asm(args) => args.asm(),
        Commands::Qemu(args) => args.qemu(),
        Commands::Gdb(args) => args.gdb(),
    }
}

/// 初始化 LFS。
fn make_git_lfs() {
    if !Git::lfs()
        .arg("version")
        .as_mut()
        .output()
        .map_or(false, |out| out.stdout.starts_with(b"git-lfs/"))
    {
        panic!("Cannot find git lfs, see https://git-lfs.github.com/ for help.");
    }
    Git::lfs().arg("install").invoke();
    Git::lfs().arg("pull").invoke();
}

/// 更新子项目。
fn git_submodule_update(init: bool) {
    Git::submodule_update(init).invoke();
}

/// 更新工具链和依赖。
fn update_all() {
    git_submodule_update(false);
    Ext::new("rustup").arg("update").invoke();
    Cargo::update().invoke();
}

/// 设置 git 代理。
fn set_git_proxy(global: bool, port: u16) {
    let dns = read_to_string("/etc/resolv.conf")
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("nameserver ")
                .and_then(|s| s.parse::<Ipv4Addr>().ok())
        })
        .expect("FAILED: detect DNS");
    let proxy = format!("socks5://{dns}:{port}");
    Git::config(global).args(&["http.proxy", &proxy]).invoke();
    Git::config(global).args(&["https.proxy", &proxy]).invoke();
    println!("git proxy = {proxy}");
}

/// 移除 git 代理。
fn unset_git_proxy(global: bool) {
    Git::config(global)
        .args(&["--unset", "http.proxy"])
        .invoke();
    Git::config(global)
        .args(&["--unset", "https.proxy"])
        .invoke();
    println!("git proxy =");
}

/// 风格检查。
fn check_style() {
    println!("Check workspace");
    Cargo::fmt().arg("--all").arg("--").arg("--check").invoke();
    Cargo::clippy().all_features().invoke();
    Cargo::doc().all_features().arg("--no-deps").invoke();

    println!("Check libos");
    Cargo::clippy()
        .package("zcore")
        .features(false, &["zircon", "libos"])
        .invoke();
    Cargo::clippy()
        .package("zcore")
        .features(false, &["linux", "libos"])
        .invoke();

    println!("Check bare-metal");
    Make::new(None)
        .arg("clippy")
        .env("ARCH", "x86_64")
        .current_dir("zCore")
        .invoke();
    Make::new(None)
        .arg("clippy")
        .env("ARCH", "riscv64")
        .env("LINUX", "1")
        .current_dir("zCore")
        .invoke();
}

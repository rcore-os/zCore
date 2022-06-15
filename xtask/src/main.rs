#![feature(path_file_prefix)]
#![feature(exit_status_error)]

#[macro_use]
extern crate clap;

use clap::Parser;
use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

#[cfg(not(target_arch = "riscv64"))]
mod dump;

mod arch;
mod build;
mod command;
mod errors;
mod linux;

use arch::{Arch, ArchArg};
use build::{AsmArgs, GdbArgs, QemuArgs};
use command::{dir, download::wget, Cargo, CommandExt, Ext, Git, Make, Tar};
use errors::XError;
use linux::LinuxRootfs;

lazy_static::lazy_static! {
    /// The path of zCore project.
    static ref PROJECT_DIR: &'static Path = Path::new(std::env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    /// The path to store arch-dependent files from network.
    static ref ARCHS: PathBuf = PROJECT_DIR.join("ignored").join("origin").join("archs");
    /// The path to store third party repos from network.
    static ref REPOS: PathBuf = PROJECT_DIR.join("ignored").join("origin").join("repos");
    /// The path to cache generated files durning processes.
    static ref TARGET: PathBuf = PROJECT_DIR.join("ignored").join("target");
}

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
    #[cfg(not(target_arch = "riscv64"))]
    Dump,

    /// First time running.
    Setup,
    /// Update rustup and cargo.
    UpdateAll,
    /// Check style
    CheckStyle,

    /// Build rootfs
    Rootfs(ArchArg),
    /// Put musl libs into rootfs
    MuslLibs(ArchArg),
    /// Put opencv libs into rootfs
    Opencv(ArchArg),
    /// Put ffmpeg libs into rootfs
    Ffmpeg(ArchArg),
    /// Put libc test into rootfs
    LibcTest(ArchArg),
    /// Put other test into rootfs
    OtherTest(ArchArg),
    /// Build image
    Image(ArchArg),

    /// Build rootfs for libos mode and put libc test in
    LibosLibcTest,
    /// Run in Linux libos mode
    LinuxLibos(LinuxLibosArg),

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

#[derive(Args)]
struct LinuxLibosArg {
    /// Command for busybox.
    #[clap(short, long)]
    pub args: String,
}

fn main() {
    use Commands::*;
    match Cli::parse().command {
        GitProxy(ProxyPort { port, global }) => {
            if let Some(port) = port {
                set_git_proxy(global, port);
            } else {
                unset_git_proxy(global);
            }
        }
        #[cfg(not(target_arch = "riscv64"))]
        Dump => dump::dump_config(),
        Setup => {
            make_git_lfs();
            git_submodule_update(true);
        }
        UpdateAll => update_all(),
        CheckStyle => check_style(),

        Rootfs(arg) => arg.linux_rootfs().make(true),
        MuslLibs(arg) => {
            arg.linux_rootfs().put_musl_libs();
        }
        Opencv(arg) => arg.linux_rootfs().put_opencv(),
        Ffmpeg(arg) => arg.linux_rootfs().put_ffmpeg(),
        LibcTest(arg) => arg.linux_rootfs().put_libc_test(),
        OtherTest(arg) => arg.linux_rootfs().put_other_test(),
        Image(arg) => arg.linux_rootfs().image(),

        LibosLibcTest => {
            libos_rootfs(true);
            libos_libc_test();
        }
        LinuxLibos(arg) => linux_libos(arg.args),

        Asm(args) => args.asm(),
        Qemu(args) => args.qemu(),
        Gdb(args) => args.gdb(),
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
    let dns = fs::read_to_string("/etc/resolv.conf")
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
    Make::new()
        .arg("clippy")
        .env("ARCH", "x86_64")
        .current_dir("zCore")
        .invoke();
    Make::new()
        .arg("clippy")
        .env("ARCH", "riscv64")
        .env("LINUX", "1")
        .current_dir("zCore")
        .invoke();
}

fn libos_rootfs(clear: bool) {
    // 下载
    const URL: &str =
        "https://github.com/YdrMaster/zCore/releases/download/dev-busybox/rootfs-libos.tar.gz";
    let origin = ARCHS.join("libos").join("rootfs-libos.tar.gz");
    dir::create_parent(&origin).unwrap();
    wget(URL, &origin);
    // 解压
    let target = TARGET.join("libos");
    fs::create_dir_all(&target).unwrap();
    Tar::xf(origin.as_os_str(), Some(&target)).invoke();
    // 拷贝
    const ROOTFS: &str = "rootfs/libos";
    if clear {
        dir::clear(ROOTFS).unwrap();
    }
    dircpy::copy_dir(target.join("rootfs"), ROOTFS).unwrap();
}

fn libos_libc_test() {
    const TARGET: &str = "rootfs/libos/libc-test";
    LinuxRootfs::new(Arch::X86_64).put_libc_test();
    dir::clear(TARGET).unwrap();
    dircpy::copy_dir("rootfs/x86_64/libc-test", TARGET).unwrap();
}

/// libos 模式执行应用程序。
fn linux_libos(args: String) {
    println!("{}", std::env!("OUT_DIR"));
    libos_rootfs(false);
    // 启动！
    Cargo::run()
        .package("zcore")
        .release()
        .features(true, ["linux"])
        .arg("--")
        .args(args.split_whitespace())
        .invoke()
}

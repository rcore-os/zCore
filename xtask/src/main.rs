use clap::{Args, Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use std::{fs::read_to_string, net::Ipv4Addr, path::Path, process::Command};

mod arch;
mod dir;
mod dump;
mod git;

use arch::Arch;

const ALPINE_ROOTFS_VERSION: &str = "3.15.4";

/// Build or test zCore.
#[derive(Parser)]
#[clap(name = "zCore configure")]
#[clap(version, about, long_about = None)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
    #[clap(flatten)]
    env: Env,
    #[clap(flatten)]
    verbose: Verbosity,
}

#[derive(Subcommand)]
enum Commands {
    /// First time running.
    Setup,
    /// Install rcore-fs-fuse.
    FsFuse,
    /// Clone libc-test.
    LibcTest,
    /// Set git proxy.
    ///
    /// Input your proxy port to set the proxy,
    /// or leave blank to unset it.
    GitProxy(ProxyPort),
    /// Update rustup and cargo.
    Update,
    /// Build rootfs
    Rootfs(Arch),
    /// Build image
    Image(Arch),
    /// Check style
    Check,
    /// Unit test
    Test,
}

#[derive(Args)]
struct Env {
    /// Build in release mode.
    #[clap(short, long, global = true)]
    release: bool,

    /// Dump build config.
    #[clap(short, long, global = true)]
    dump: bool,
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
    let cli = Cli::parse();

    if cli.env.dump {
        dump::dump_config();
    }

    match cli.command {
        Commands::Setup => {
            check_git_lfs();
            make_git_lfs();
            install_fs_fuse();
        }
        Commands::FsFuse => install_fs_fuse(),
        Commands::LibcTest => clone_libc_test(),
        Commands::GitProxy(ProxyPort { port, global }) => {
            if let Some(port) = port {
                set_git_proxy(global, port);
            } else {
                unset_git_proxy(global);
            }
        }
        Commands::Update => update_rustup_cargo(),
        Commands::Rootfs(arch) => arch.rootfs(),
        Commands::Image(arch) => arch.image(),
        Commands::Check => check_style(),
        Commands::Test => {}
    }
}

/// 检查 LFS 程序是否存在。
fn check_git_lfs() {
    if let Ok(true) = git::lfs()
        .arg("version")
        .output()
        .map(|out| out.stdout.starts_with(b"git-lfs/"))
    {
    } else {
        panic!("Cannot find git lfs, see https://git-lfs.github.com/ for help.");
    }
}

/// 初始化 LFS。
fn make_git_lfs() {
    if !git::lfs().arg("install").status().unwrap().success() {
        panic!("FAILED: git lfs install")
    }

    if !git::lfs().arg("pull").status().unwrap().success() {
        panic!("FAILED: git lfs pull")
    }
}

/// 更新工具链。
fn update_rustup_cargo() {
    if !Command::new("rustup")
        .arg("update")
        .status()
        .unwrap()
        .success()
    {
        panic!("FAILED: rustup update");
    }
    if !Command::new("cargo")
        .arg("update")
        .status()
        .unwrap()
        .success()
    {
        panic!("FAILED: cargo update");
    }
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
    #[rustfmt::skip]
    let git = git::config(global)
        .arg("http.proxy").arg(&proxy)
        .status().unwrap();
    if !git.success() {
        panic!("FAILED: git config --unset http.proxy");
    }
    #[rustfmt::skip]
    let git = git::config(global)
        .arg("https.proxy").arg(&proxy)
        .status().unwrap();
    if !git.success() {
        panic!("FAILED: git config --unset https.proxy");
    }
    println!("git proxy = {proxy}");
}

/// 移除 git 代理。
fn unset_git_proxy(global: bool) {
    #[rustfmt::skip]
    let git = git::config(global)
        .arg("--unset").arg("http.proxy")
        .status().unwrap();
    if !git.success() {
        panic!("FAILED: git config --unset http.proxy");
    }
    #[rustfmt::skip]
    let git = git::config(global)
        .arg("--unset").arg("https.proxy")
        .status().unwrap();
    if !git.success() {
        panic!("FAILED: git config --unset https.proxy");
    }
    println!("git proxy =");
}

/// 风格检查。
fn check_style() {
    println!("fmt -----------------------------------------");
    #[rustfmt::skip]
    Command::new("cargo").arg("fmt")
        .arg("--all")
        .arg("--")
        .arg("--check")
        .status()
        .unwrap();
    println!("clippy --------------------------------------");
    #[rustfmt::skip]
    Command::new("cargo").arg("clippy")
        .arg("--all-features")
        .status()
        .unwrap();
    println!("clippy x86_64 zircon smp=1 ------------------");
    #[rustfmt::skip]
    Command::new("cargo").arg("clippy")
        .arg("--no-default-features")
        .arg("--features").arg("zircon")
        .arg("--target").arg("x86_64.json")
        .arg("-Z").arg("build-std=core,alloc")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem")
        .current_dir("zCore")
        .env("SMP", "1")
        .status()
        .unwrap();
    println!("clippy riscv64 linux smp=4 ------------------");
    #[rustfmt::skip]
    Command::new("cargo").arg("clippy")
        .arg("--no-default-features")
        .arg("--features").arg("linux board-qemu")
        .arg("--target").arg("riscv64.json")
        .arg("-Z").arg("build-std=core,alloc")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem")
        .current_dir("zCore")
        .env("SMP", "4")
        .env("PLATFORM", "board-qemu")
        .status()
        .unwrap();
}

/// 安装 rcore-fs-fuse。
fn install_fs_fuse() {
    if let Ok(true) = Command::new("rcore-fs-fuse")
        .arg("--version")
        .output()
        .map(|out| out.stdout.starts_with(b"rcore-fs-fuse"))
    {
        println!("Rcore-fs-fuse is already installed.");
        return;
    }
    #[rustfmt::skip]
    let install = Command::new("cargo")
        .arg("install").arg("rcore-fs-fuse")
        .arg("--git").arg("https://github.com/rcore-os/rcore-fs")
        .arg("--rev").arg("1a3246b")
        .arg("--force")
        .status();
    if !install.unwrap().success() {
        panic!("FAILED: install rcore-fs-fuse");
    }
}

/// 克隆 libc-test.
fn clone_libc_test() {
    const DIR: &str = "ignored/libc-test";
    const URL: &str = "https://github.com/rcore-os/libc-test.git";

    if Path::new(DIR).is_dir() {
        let pull = git::pull().current_dir(DIR).status();
        if !pull.unwrap().success() {
            panic!("FAILED: git pull");
        }
    } else {
        dir::clear(DIR).unwrap();
        let clone = git::clone(URL, Some(DIR)).status();
        if !clone.unwrap().success() {
            panic!("FAILED: git clone {URL}");
        }
    }
}

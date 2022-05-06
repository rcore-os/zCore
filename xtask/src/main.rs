#![feature(path_file_prefix)]

use clap::{Args, Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use std::{fs::read_to_string, net::Ipv4Addr, process::Command};

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
    /// Set git proxy.
    ///
    /// Input your proxy port to set the proxy,
    /// or leave blank to unset it.
    GitProxy(ProxyPort),
    /// Update rustup and cargo.
    UpdateAll,
    /// Check style
    CheckStyle,
    /// Build rootfs
    Rootfs(Arch),
    /// Put libc-test.
    LibcTest(Arch),
    /// Build image
    Image(Arch),
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
            make_git_lfs();
            git_submodule_update(true);
        }
        Commands::GitProxy(ProxyPort { port, global }) => {
            if let Some(port) = port {
                set_git_proxy(global, port);
            } else {
                unset_git_proxy(global);
            }
        }
        Commands::UpdateAll => update_all(),
        Commands::Rootfs(arch) => arch.rootfs(),
        Commands::LibcTest(arch) => arch.libc_test(),
        Commands::Image(arch) => arch.image(),
        Commands::CheckStyle => check_style(),
        Commands::Test => {}
    }
}

/// 初始化 LFS。
fn make_git_lfs() {
    if !git::lfs()
        .arg("version")
        .output()
        .map_or(false, |out| out.stdout.starts_with(b"git-lfs/"))
    {
        panic!("Cannot find git lfs, see https://git-lfs.github.com/ for help.");
    }
    if !git::lfs().arg("install").status().unwrap().success() {
        panic!("FAILED: git lfs install")
    }

    if !git::lfs().arg("pull").status().unwrap().success() {
        panic!("FAILED: git lfs pull")
    }
}

/// 更新子项目。
fn git_submodule_update(init: bool) {
    if !git::submodule_update(init).status().unwrap().success() {
        panic!("FAILED: git submodule update --init");
    }
}

/// 更新工具链和依赖。
fn update_all() {
    git_submodule_update(false);
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

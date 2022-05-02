use clap::{Args, Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use std::{fs::read_to_string, net::Ipv4Addr, process::Command};

mod dump;

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
    Init,
    /// Set git proxy.
    ///
    /// Input your proxy port to set the proxy,
    /// or leave blank to unset it.
    GitProxy(ProxyPort),
    /// Update rustup and cargo.
    Update,
    /// Build rootfs
    Rootfs,
    /// Build image
    Image,
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

#[derive(Args, Debug)]
struct ProxyPort {
    /// Proxy port.
    #[clap(long)]
    port: Option<u16>,
}

fn main() {
    let cli = Cli::parse();

    if cli.env.dump {
        dump::dump_config();
    }

    match cli.command {
        Commands::Init => {
            check_git_lfs();
            make_git_lfs();
        }
        Commands::GitProxy(ProxyPort { port }) => {
            if let Some(port) = port {
                set_proxy(port);
            } else {
                unset_proxy();
            }
        }
        Commands::Update => update(),
        Commands::Rootfs => {}
        Commands::Image => {}
        Commands::Check => check(),
        Commands::Test => {}
    }
}

fn check_git_lfs() {
    if let Ok(true) = Command::new("git")
        .arg("lfs")
        .arg("version")
        .output()
        .map(|out| out.stdout.starts_with(b"git-lfs/"))
    {
    } else {
        panic!("Cannot find git lfs, see https://git-lfs.github.com/ for help.");
    }
}

fn make_git_lfs() {
    if !Command::new("git")
        .arg("lfs")
        .arg("install")
        .status()
        .unwrap()
        .success()
    {
        panic!("FAILED: git lfs install")
    }

    if !Command::new("git")
        .arg("lfs")
        .arg("pull")
        .status()
        .unwrap()
        .success()
    {
        panic!("FAILED: git lfs pull")
    }
}

fn update() {
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

fn set_proxy(port: u16) {
    let dns = read_to_string("/etc/resolv.conf")
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("nameserver ")
                .and_then(|s| s.parse::<Ipv4Addr>().ok())
        })
        .expect("FAILED: detect DNS");
    let proxy = format!("socks5://{dns}:{port}");
    if !Command::new("git")
        .arg("config")
        .arg("http.proxy")
        .arg(&proxy)
        .status()
        .unwrap()
        .success()
    {
        panic!("FAILED: git config --unset http.proxy");
    }
    if !Command::new("git")
        .arg("config")
        .arg("https.proxy")
        .arg(&proxy)
        .status()
        .unwrap()
        .success()
    {
        panic!("FAILED: git config --unset https.proxy");
    }
    println!("git proxy = {proxy}");
}

fn unset_proxy() {
    if !Command::new("git")
        .arg("config")
        .arg("--unset")
        .arg("http.proxy")
        .status()
        .unwrap()
        .success()
    {
        panic!("FAILED: git config --unset http.proxy");
    }
    if !Command::new("git")
        .arg("config")
        .arg("--unset")
        .arg("https.proxy")
        .status()
        .unwrap()
        .success()
    {
        panic!("FAILED: git config --unset https.proxy");
    }
    println!("git proxy =");
}

fn check() {
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
        .env("PLATFORM", "board-qemu") .status()
        .unwrap();
}

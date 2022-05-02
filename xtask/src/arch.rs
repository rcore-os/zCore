use super::ALPINE_ROOTFS_VERSION;
use clap::{Args, Subcommand};
use std::{fs::create_dir_all, path::PathBuf, process::Command};

#[derive(Args)]
pub struct Arch {
    #[clap(subcommand)]
    command: ArchCommands,
}

#[derive(Subcommand)]
enum ArchCommands {
    #[clap(name = "riscv64")]
    Riscv64,
    #[clap(name = "x86_64")]
    X86_64,
}

impl Arch {
    /// 获取 alpine 镜像。
    pub fn wget_alpine(&self) {
        let (local_path, web_url) = match self.command {
            ArchCommands::Riscv64 => {
                const ARCH: &str = "riscv64";
                const FILE_NAME: &str = "prebuild.tar.xz";

                let local_path = PathBuf::from(format!("prebuilt/linux/{ARCH}/{FILE_NAME}"));
                if local_path.exists() {
                    return;
                }
                let web_url = format!("https://github.com/rcore-os/libc-test-prebuilt/releases/download/0.1/{FILE_NAME}");
                (local_path, web_url)
            }
            ArchCommands::X86_64 => {
                const ARCH: &str = "x86_64";
                let file_name = format!("alpine-minirootfs-{ALPINE_ROOTFS_VERSION}-{ARCH}.tar.gz");

                let local_path = PathBuf::from(format!("prebuilt/linux/{ARCH}/{file_name}"));
                if local_path.exists() {
                    return;
                }
                let web_url = format!("https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/{ARCH}/{file_name}");
                (local_path, web_url)
            }
        };
        create_dir_all(local_path.parent().unwrap()).unwrap();

        let wget = Command::new("wget")
            .arg(&web_url)
            .arg("-O")
            .arg(local_path)
            .status();
        if !wget.unwrap().success() {
            panic!("FAILED: wget {web_url}");
        }
    }
}

use super::ALPINE_ROOTFS_VERSION;
use clap::{Args, Subcommand};
use std::{
    fs::{create_dir, create_dir_all, remove_dir_all},
    io::ErrorKind,
    path::PathBuf,
    process::Command,
};

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
    fn wget_alpine(&self) {
        let (local_path, web_url) = match self.command {
            ArchCommands::Riscv64 => {
                const ARCH: &str = "riscv64";
                const FILE_NAME: &str = "minirootfs.tar.xz";
                const WEB_URL: &str = "https://github.com/rcore-os/libc-test-prebuilt/releases/download/0.1/prebuild.tar.xz";

                let local_path = PathBuf::from(format!("prebuilt/linux/{ARCH}/{FILE_NAME}"));
                if local_path.exists() {
                    return;
                }
                (local_path, WEB_URL.into())
            }
            ArchCommands::X86_64 => {
                const ARCH: &str = "x86_64";
                const FILE_NAME: &str = "minirootfs.tar.gz";
                const WEBSITE: &str =
                    "https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases";

                let local_path = PathBuf::from(format!("prebuilt/linux/{ARCH}/{FILE_NAME}"));
                if local_path.exists() {
                    return;
                }
                (
                    local_path,
                    format!(
                        "{WEBSITE}/{ARCH}/alpine-minirootfs-{ALPINE_ROOTFS_VERSION}-{ARCH}.tar.gz"
                    ),
                )
            }
        };

        create_dir_all(local_path.parent().unwrap()).unwrap();

        #[rustfmt::skip]
        let wget = Command::new("wget")
            .arg(&web_url)
            .arg("-O").arg(local_path)
            .status();
        if !wget.unwrap().success() {
            panic!("FAILED: wget {web_url}");
        }
    }

    pub fn rootfs(&self) {
        self.wget_alpine();
        match self.command {
            ArchCommands::Riscv64 => {
                const DIR: &str = "riscv_rootfs";
                const ARCH: &str = "riscv64";

                rm_rf(DIR).unwrap();
                create_dir(DIR).unwrap();
                #[rustfmt::skip]
                Command::new("tar")
                    .arg("xf").arg(format!("prebuilt/linux/{ARCH}/minirootfs.tar.xz"))
                    .arg("-C").arg(DIR)
                    .arg("--strip-components").arg("1")
                    .status().unwrap();
                #[rustfmt::skip]
                let ln = Command::new("ln")
                    .arg("-s")
                    .arg("busybox")
                    .arg("riscv_rootfs/bin/ls")
                    .status().unwrap();
                if !ln.success() {
                    panic!("FAILED: ln -s busybox riscv_rootfs/bin/ls");
                }
            }
            ArchCommands::X86_64 => {
                const DIR: &str = "rootfs";
                const ARCH: &str = "x86_64";

                rm_rf(DIR).unwrap();
                create_dir(DIR).unwrap();
                #[rustfmt::skip]
                Command::new("tar")
                    .arg("xf").arg(format!("prebuilt/linux/{ARCH}/minirootfs.tar.gz"))
                    .arg("-C").arg(DIR)
                    .status().unwrap();
                // libc-libos.so (convert syscall to function call) is from https://github.com/rcore-os/musl/tree/rcore
                std::fs::copy(
                    "prebuilt/linux/libc-libos.so",
                    format!("{DIR}/lib/ld-musl-{ARCH}.so.1"),
                )
                .unwrap();
                // 	@for VAR in $(BASENAMES); do gcc $(TEST_DIR)$$VAR.c -o $(DEST_DIR)$$VAR $(CFLAG); done
            }
        }
    }
}

fn rm_rf(dir: &str) -> std::io::Result<()> {
    match remove_dir_all(dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        res => res,
    }
}

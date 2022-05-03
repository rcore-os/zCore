use super::ALPINE_ROOTFS_VERSION;
use clap::{Args, Subcommand};
use std::{
    ffi::OsStr,
    fs::{create_dir_all, remove_dir_all},
    io::ErrorKind,
    path::{Path, PathBuf},
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

                {
                    clear(DIR).unwrap();
                    let tar = detect(&format!("prebuilt/linux/{ARCH}"), "minirootfs").unwrap();
                    #[rustfmt::skip]
                    let res = tar_xf(&tar, Some(DIR))
                        .arg("--strip-components").arg("1")
                        .status().unwrap();
                    if !res.success() {
                        panic!("FAILED: tar xf {tar:?}");
                    }
                }
                {
                    #[rustfmt::skip]
                    let ln = Command::new("ln")
                        .arg("-s").arg("busybox").arg("riscv_rootfs/bin/ls")
                        .status().unwrap();
                    if !ln.success() {
                        panic!("FAILED: ln -s busybox riscv_rootfs/bin/ls");
                    }
                }
                {
                    const DIR: &str = "toolchain";
                    let name = format!("{ARCH}-linux-musl-cross");
                    let dir = format!("{DIR}/{name}");
                    let tgz = format!("{dir}.tgz");

                    if !Path::new(&tgz).exists() {
                        clear(DIR).unwrap();
                        let url = format!("https://musl.cc/{name}.tgz");
                        #[rustfmt::skip]
                        let wget = Command::new("wget")
                            .arg(&url)
                            .arg("-O").arg(&tgz)
                            .status().unwrap();
                        if !wget.success() {
                            panic!("FAILED: wget {url}");
                        }
                    } else {
                        rm_rf(&dir).unwrap();
                    }
                    if !tar_xf(&tgz, Some(DIR)).status().unwrap().success() {
                        panic!("FAILED: tar xf {tgz}");
                    }
                }
            }
            ArchCommands::X86_64 => {
                const DIR: &str = "rootfs";
                const ARCH: &str = "x86_64";

                {
                    clear(DIR).unwrap();
                    let tar = detect(&format!("prebuilt/linux/{ARCH}"), "minirootfs").unwrap();
                    if !tar_xf(&tar, Some(DIR)).status().unwrap().success() {
                        panic!("FAILED: tar xf {tar:?}");
                    }
                }
                {
                    // libc-libos.so (convert syscall to function call) is from https://github.com/rcore-os/musl/tree/rcore
                    std::fs::copy(
                        "prebuilt/linux/libc-libos.so",
                        format!("{DIR}/lib/ld-musl-{ARCH}.so.1"),
                    )
                    .unwrap();
                }
                {
                    // 	@for VAR in $(BASENAMES); do gcc $(TEST_DIR)$$VAR.c -o $(DEST_DIR)$$VAR $(CFLAG); done
                }
            }
        }
    }
}

/// 递归删除指定目录。
fn rm_rf(dir: &(impl AsRef<Path> + ?Sized)) -> std::io::Result<()> {
    match remove_dir_all(dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        res => res,
    }
}

/// 清理指定目录。
fn clear(dir: &(impl AsRef<Path> + ?Sized)) -> std::io::Result<()> {
    rm_rf(dir)?;
    create_dir_all(dir)?;
    Ok(())
}

/// 解压指定文件。
fn tar_xf(src: &impl AsRef<OsStr>, dst: Option<&str>) -> Command {
    let mut cmd = Command::new("tar");
    cmd.arg("xf").arg(src);
    if let Some(dst) = dst {
        cmd.arg("-C").arg(dst);
    }
    cmd
}

/// 在指定目录根据部分文件名找到指定文件。
fn detect(dir: &impl AsRef<OsStr>, prefix: &str) -> Option<PathBuf> {
    Path::new(dir)
        .read_dir()
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map_or(false, |ty| ty.is_file()))
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|s| s.to_str())
                .map_or(false, |s| s.starts_with(prefix))
        })
}

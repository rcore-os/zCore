use super::{dir, git, ALPINE_ROOTFS_VERSION};
use clap::{Args, Subcommand};
use dircpy::copy_dir;
use std::{
    ffi::OsStr,
    fs,
    io::Write,
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

        fs::create_dir_all(local_path.parent().unwrap()).unwrap();

        #[rustfmt::skip]
        let wget = Command::new("wget")
            .arg(&web_url)
            .arg("-O").arg(local_path)
            .status();
        if !wget.unwrap().success() {
            panic!("FAILED: wget {web_url}");
        }
    }

    /// 构造启动内存文件系统 rootfs。
    ///
    /// 将在文件系统中放置必要的库文件，并下载用于交叉编译的工具链。
    pub fn rootfs(&self) {
        self.wget_alpine();
        match self.command {
            ArchCommands::Riscv64 => {
                const DIR: &str = "riscv_rootfs";
                const ARCH: &str = "riscv64";

                {
                    dir::clear(DIR).unwrap();
                    let tar = dir::detect(&format!("prebuilt/linux/{ARCH}"), "minirootfs").unwrap();
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
                        dir::clear(DIR).unwrap();
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
                        dir::rm(&dir).unwrap();
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
                    dir::clear(DIR).unwrap();
                    let tar = dir::detect(&format!("prebuilt/linux/{ARCH}"), "minirootfs").unwrap();
                    if !tar_xf(&tar, Some(DIR)).status().unwrap().success() {
                        panic!("FAILED: tar xf {tar:?}");
                    }
                }
                {
                    // libc-libos.so (convert syscall to function call) is from https://github.com/rcore-os/musl/tree/rcore
                    fs::copy(
                        "prebuilt/linux/libc-libos.so",
                        format!("{DIR}/lib/ld-musl-{ARCH}.so.1"),
                    )
                    .unwrap();
                }
                {
                    const TEST_DIR: &str = "linux-syscall/test";
                    const DEST_DIR: &str = "rootfs/bin/";
                    // for linux syscall tests
                    fs::read_dir(TEST_DIR)
                        .unwrap()
                        .filter_map(|res| res.ok())
                        .map(|entry| entry.path())
                        .filter(|path| path.extension().map_or(false, |ext| ext == OsStr::new("c")))
                        .for_each(|c| {
                            let o = format!(
                                "{DEST_DIR}/{}",
                                c.file_prefix().and_then(|s| s.to_str()).unwrap()
                            );
                            #[rustfmt::skip]
                            let gcc = Command::new("gcc").arg(&c)
                                .arg("-o").arg(o)
                                .arg("-Wl,--dynamic-linker=/lib/ld-musl-x86_64.so.1")
                                .status().unwrap();
                            if !gcc.success() {
                                panic!("FAILED: gcc {c:?}");
                            }
                        });
                }
            }
        }
    }

    /// 将 libc-test 放入 rootfs。
    pub fn libc_test(&self) {
        clone_libc_test();
        match self.command {
            ArchCommands::Riscv64 => {
                const DIR: &str = "riscv_rootfs/libc-test";
                const PRE: &str = "riscv_rootfs/libc-test-prebuild";
                fs::rename(DIR, PRE).unwrap();
                copy_dir("ignored/libc-test", DIR).unwrap();
                fs::copy(format!("{DIR}/config.mak.def"), format!("{DIR}/config.mak")).unwrap();
                let make = Command::new("make")
                    .arg("-j")
                    .env("ARCH", "riscv64")
                    .env("CROSS_COMPILE", "riscv64-linux-musl-")
                    .current_dir(DIR)
                    .status();
                if !make.unwrap().success() {
                    panic!("FAILED: make -j");
                }
                fs::copy(
                    format!("{PRE}/functional/tls_align-static.exe"),
                    format!("{DIR}/src/functional/tls_align-static.exe"),
                )
                .unwrap();
                dir::rm(PRE).unwrap();
            }
            ArchCommands::X86_64 => {
                const DIR: &str = "rootfs/libc-test";

                dir::rm(DIR).unwrap();
                copy_dir("ignored/libc-test", DIR).unwrap();
                fs::copy(format!("{DIR}/config.mak.def"), format!("{DIR}/config.mak")).unwrap();
                fs::OpenOptions::new()
                    .append(true)
                    .open(format!("{DIR}/config.mak"))
                    .unwrap()
                    .write_all(b"CC := musl-gcc")
                    .unwrap();
                if !Command::new("make")
                    .arg("-j")
                    .current_dir(DIR)
                    .status()
                    .unwrap()
                    .success()
                {
                    panic!("FAILED: make -j");
                }
            }
        }
    }

    /// 生成镜像。
    pub fn image(&self) {
        install_fs_fuse();
        match self.command {
            ArchCommands::Riscv64 => {
                // TODO 后续
            }
            ArchCommands::X86_64 => {
                const ARCH: &str = "x86_64";
                const TMP_ROOTFS: &str = "/tmp/rootfs";
                const ROOTFS_LIB: &str = "rootfs/lib";

                // ld-musl-x86_64.so.1 替换为预编译中的版本
                dir::clear(TMP_ROOTFS).unwrap();
                let tar = dir::detect(&format!("prebuilt/linux/{ARCH}"), "minirootfs").unwrap();
                if !tar_xf(&tar, Some(TMP_ROOTFS)).status().unwrap().success() {
                    panic!("FAILED: tar xf {tar:?}");
                }
                dir::clear(ROOTFS_LIB).unwrap();
                fs::copy(
                    format!("{TMP_ROOTFS}/lib/ld-musl-x86_64.so.1"),
                    format!("{ROOTFS_LIB}/ld-musl-x86_64.so.1"),
                )
                .unwrap();

                // TODO 后续
            }
        }
    }
}

/// 构造将归档文件 `src` 释放到 `dst` 目录下的命令。
///
/// 本身不会产生异常，因为命令还没有执行。
/// 但若 `src` 不是存在的归档文件，或 `dst` 不是存在的目录，将在命令执行时产生对应异常。
fn tar_xf(src: &impl AsRef<OsStr>, dst: Option<&str>) -> Command {
    let mut cmd = Command::new("tar");
    cmd.arg("xf").arg(src);
    if let Some(dst) = dst {
        cmd.arg("-C").arg(dst);
    }
    cmd
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

﻿use crate::{
    cargo::Cargo, dir, git::Git, wget::wget, CommandExt, ALPINE_ROOTFS_VERSION, ALPINE_WEBSITE,
};
use dircpy::copy_dir;
use std::{
    ffi::OsStr,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Args)]
pub(super) struct Arch {
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
    /// 构造启动内存文件系统 rootfs。
    ///
    /// 将在文件系统中放置必要的库文件，并下载用于交叉编译的工具链。
    pub fn rootfs(&self, clear: bool) {
        self.wget_alpine();
        match self.command {
            ArchCommands::Riscv64 => {
                const DIR: &str = "riscv_rootfs";
                const ARCH: &str = "riscv64";

                let dir = Path::new(DIR);
                if dir.is_dir() && !clear {
                    return;
                }
                dir::clear(dir).unwrap();
                let tar = dir::detect(&format!("prebuilt/linux/{ARCH}"), "minirootfs").unwrap();
                #[rustfmt::skip]
                tar_xf(&tar, Some(DIR))
                    .arg("--strip-components").arg("1")
                    .status().unwrap()
                    .exit_ok().expect("FAILED: tar xf {tar:?}");
                #[rustfmt::skip]
                Command::new("ln")
                    .arg("-s").arg("busybox").arg("riscv_rootfs/bin/ls")
                    .status().unwrap()
                    .exit_ok().expect("FAILED: ln -s busybox riscv_rootfs/bin/ls");
            }
            ArchCommands::X86_64 => {
                const DIR: &str = "rootfs";
                const ARCH: &str = "x86_64";

                let dir = Path::new(DIR);
                if dir.is_dir() && !clear {
                    return;
                }
                {
                    dir::clear(DIR).unwrap();
                    let tar = dir::detect(&format!("prebuilt/linux/{ARCH}"), "minirootfs").unwrap();
                    tar_xf(&tar, Some(DIR))
                        .status()
                        .unwrap()
                        .exit_ok()
                        .expect("FAILED: tar xf {tar:?}");
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
                            Command::new("gcc").arg(&c)
                                .arg("-o").arg(o)
                                .arg("-Wl,--dynamic-linker=/lib/ld-musl-x86_64.so.1")
                                .status().unwrap()
                                .exit_ok().expect("FAILED: gcc {c:?}");
                        });
                }
            }
        }
    }

    /// 将 libc-test 放入 rootfs。
    pub fn libc_test(&self) {
        self.rootfs(false);
        clone_libc_test();
        match self.command {
            ArchCommands::Riscv64 => {
                const DIR: &str = "riscv_rootfs/libc-test";
                const PRE: &str = "riscv_rootfs/libc-test-prebuild";

                riscv64_linux_musl_cross();
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
                    .write_all(b"CC := musl-gcc\nAR := ar\nRANLIB := ranlib")
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
        self.rootfs(false);
        install_fs_fuse();
        let image = match self.command {
            ArchCommands::Riscv64 => {
                const ARCH: &str = "riscv64";

                let image = format!("zCore/{ARCH}.img");
                #[rustfmt::skip]
                let fuse = Command::new("rcore-fs-fuse")
                    .arg(&image).arg("riscv_rootfs").arg("zip")
                    .status().unwrap();
                if !fuse.success() {
                    panic!("FAILED: rcore-fs-fuse");
                }
                image
            }
            ArchCommands::X86_64 => {
                const ARCH: &str = "x86_64";
                const TMP_ROOTFS: &str = "/tmp/rootfs";
                const ROOTFS_LIB: &str = "rootfs/lib";

                // ld-musl-x86_64.so.1 替换为适用 bare-matel 的版本
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

                let image = format!("zCore/{ARCH}.img");
                #[rustfmt::skip]
                let fuse = Command::new("rcore-fs-fuse")
                    .arg(&image).arg("rootfs").arg("zip")
                    .status().unwrap();
                if !fuse.success() {
                    panic!("FAILED: rcore-fs-fuse");
                }
                // ld-musl-x86_64.so.1 替换为适用 libos 的版本
                // # libc-libos.so (convert syscall to function call) is from https://github.com/rcore-os/musl/tree/rcore
                fs::copy(
                    "prebuilt/linux/libc-libos.so",
                    format!("{ROOTFS_LIB}/ld-musl-x86_64.so.1"),
                )
                .unwrap();
                image
            }
        };
        #[rustfmt::skip]
        let resize = Command::new("qemu-img")
            .arg("resize").arg(image).arg("+5M")
            .status().unwrap();
        if !resize.success() {
            panic!("FAILED: qemu-img resize");
        }
    }

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

                let local_path = PathBuf::from(format!("prebuilt/linux/{ARCH}/{FILE_NAME}"));
                if local_path.exists() {
                    return;
                }
                (
                    local_path,
                    format!(
                        "{ALPINE_WEBSITE}/{ARCH}/alpine-minirootfs-{ALPINE_ROOTFS_VERSION}-{ARCH}.tar.gz"
                    ),
                )
            }
        };

        fs::create_dir_all(local_path.parent().unwrap()).unwrap();
        wget(&web_url, &local_path);
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
    } else {
        Cargo::new("install")
            .args(&["install", "rcore-fs-fuse"])
            .args(&["--git", "https://github.com/rcore-os/rcore-fs"])
            .args(&["--rev", "1a3246b"])
            .arg("--force")
            .expect("FAILED: install rcore-fs-fuse");
    }
}

/// 克隆 libc-test.
fn clone_libc_test() {
    const DIR: &str = "ignored/libc-test";
    const URL: &str = "https://github.com/rcore-os/libc-test.git";

    if Path::new(DIR).is_dir() {
        Git::pull().current_dir(DIR).expect("FAILED: git pull");
    } else {
        dir::clear(DIR).unwrap();
        Git::clone(URL, Some(DIR)).expect(&format!("FAILED: git clone {URL}"));
    }
}

/// 下载 riscv64-musl 工具链。
fn riscv64_linux_musl_cross() {
    const DIR: &str = "ignored";
    const NAME: &str = "riscv64-linux-musl-cross";
    let dir = format!("{DIR}/{NAME}");
    let tgz = format!("{dir}.tgz");

    wget(&format!("https://musl.cc/{NAME}.tgz"), &tgz);
    dir::rm(&dir).unwrap();
    if !tar_xf(&tgz, Some(DIR)).status().unwrap().success() {
        panic!("FAILED: tar xf {tgz}");
    }
}
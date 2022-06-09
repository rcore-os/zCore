use crate::{
    command::{dir, download::wget, CommandExt, Tar},
    Arch, ALPINE_ROOTFS_VERSION, ALPINE_WEBSITE,
};
use std::{fs, os::unix, path::PathBuf};

mod image;
mod test;

lazy_static::lazy_static! {
    static ref LIBOS_MUSL_LIBC_PATH: PathBuf = Arch::X86_64.origin().join("libc-libos.so");
}

pub(crate) struct LinuxRootfs(Arch);

impl LinuxRootfs {
    /// 生成指定架构的 linux rootfs 操作对象。
    #[inline]
    pub const fn new(arch: Arch) -> Self {
        Self(arch)
    }

    /// 构造启动内存文件系统 rootfs。
    /// 对于 x86_64，这个文件系统可用于 libos 启动。
    /// 若设置 `clear`，将清除已存在的目录。
    pub fn make(&self, clear: bool) {
        // 若已存在且不需要清空，可以直接退出
        let dir = self.path();
        if dir.is_dir() && !clear {
            return;
        }
        // 下载压缩文件并解压
        let src = self.prebuild_rootfs();
        // 创建目标目录
        dir::clear(&dir).unwrap();
        fs::create_dir(dir.join("bin")).unwrap();
        fs::create_dir(dir.join("lib")).unwrap();
        // 拷贝 busybox
        fs::copy(src.join("bin/busybox"), dir.join("bin/busybox")).unwrap();
        // 拷贝 libc.so
        let libc_so = format!("lib/ld-musl-{arch}.so.1", arch = self.0.name());
        let so = match self.0 {
            Arch::Riscv64 | Arch::Aarch64 => src.join(&libc_so),
            Arch::X86_64 => {
                // 下载适用于 libos 的 musl libc so。
                const URL:&str = "https://github.com/rcore-os/libc-test-prebuilt/releases/download/master/libc-libos.so";
                wget(URL, LIBOS_MUSL_LIBC_PATH.as_path());
                LIBOS_MUSL_LIBC_PATH.clone()
            }
        };
        fs::copy(so, dir.join(libc_so)).unwrap();
        // 为常用功能建立符号链接
        const SH: &[&str] = &[
            "cat", "cp", "echo", "false", "grep", "gzip", "kill", "ln", "ls", "mkdir", "mv",
            "pidof", "ping", "ping6", "printenv", "ps", "pwd", "rm", "rmdir", "sh", "sleep",
            "stat", "tar", "touch", "true", "uname", "usleep", "watch",
        ];
        let bin = dir.join("bin");
        for sh in SH {
            unix::fs::symlink("busybox", bin.join(sh)).unwrap();
        }
    }

    /// 指定架构的 rootfs 路径。
    #[inline]
    fn path(&self) -> PathBuf {
        PathBuf::from_iter(["rootfs", self.0.name()])
    }

    /// 下载并解压 minirootfs。
    fn prebuild_rootfs(&self) -> PathBuf {
        // 构造压缩文件路径
        let file_name = match self.0 {
            Arch::Riscv64 => "minirootfs.tar.xz",
            Arch::X86_64 | Arch::Aarch64 => "minirootfs.tar.gz",
        };
        let tar = self.0.origin().join(file_name);
        // 若压缩文件不存在，需要下载
        if !tar.exists() {
            let url = match self.0 {
                Arch::Riscv64 => String::from("https://github.com/rcore-os/libc-test-prebuilt/releases/download/0.1/prebuild.tar.xz"),
                Arch::X86_64 => format!("{ALPINE_WEBSITE}/x86_64/alpine-minirootfs-{ALPINE_ROOTFS_VERSION}-x86_64.tar.gz"),
                Arch::Aarch64 => format!("{ALPINE_WEBSITE}/aarch64/alpine-minirootfs-{ALPINE_ROOTFS_VERSION}-aarch64.tar.gz")
            };
            wget(url, &tar);
        }
        // 解压到目标路径
        let dir = self.0.target().join("rootfs");
        dir::clear(&dir).unwrap();
        let mut tar = Tar::xf(&tar, Some(&dir));
        match self.0 {
            Arch::Riscv64 => tar.args(&["--strip-components", "1"]).invoke(),
            Arch::X86_64 | Arch::Aarch64 => tar.invoke(),
        }
        dir
    }
}

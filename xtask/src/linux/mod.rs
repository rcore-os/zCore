use crate::{
    command::{dir, download::wget, CommandExt, Tar},
    Arch, ALPINE_ROOTFS_VERSION, ALPINE_WEBSITE,
};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    os::unix,
    path::PathBuf,
};

mod image;
mod opencv;
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
        match self.0 {
            Arch::Aarch64 => {
                let aarch64_file = "Aarch64_firmware.zip";
                let aarch64_tar = self.0.origin().join(aarch64_file);
                if !aarch64_tar.exists() {
                    let url = "https://github.com/Luchangcheng2333/rayboot/releases/download/2.0.0/aarch64_firmware.tar.gz";
                    wget(url, &aarch64_tar);
                }
                let fw_dir = self.0.target().join("firmware");
                dir::clear(&fw_dir).unwrap();
                let mut aarch64_tar = Tar::xf(&aarch64_tar, Some(&fw_dir));
                aarch64_tar.invoke();
                let boot_dir = "zCore/disk/EFI/Boot";
                fs::create_dir_all(boot_dir).ok();
                fs::copy(fw_dir.join("aarch64_uefi.efi"), boot_dir);
                fs::copy(fw_dir.join("Boot.json"), boot_dir);
            }
            _ => {}
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

/// 下载 musl 工具链，返回工具链路径。
fn linux_musl_cross(arch: Arch) -> PathBuf {
    let name = format!("{}-linux-musl-cross", arch.name().to_lowercase());
    let name: &str = name.as_str();

    let origin = arch.origin();
    let target = arch.target();

    let tgz = origin.join(format!("{name}.tgz"));
    let dir = target.join(name);

    dir::rm(&dir).unwrap();
    wget(format!("https://musl.cc/{name}.tgz"), &tgz);
    Tar::xf(&tgz, Some(target)).invoke();

    // 将交叉工具链加入 PATH 环境变量
    env::current_dir().unwrap().join(dir).join("bin")
}

/// 为 PATH 环境变量附加路径。
fn join_path_env<I, S>(paths: I) -> OsString
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut path = OsString::new();
    let mut first = true;
    if let Ok(current) = env::var("PATH") {
        path.push(current);
        first = false;
    }
    for item in paths {
        if first {
            first = false;
        } else {
            path.push(":");
        }
        path.push(item);
    }
    path
}

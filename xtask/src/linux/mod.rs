use crate::{
    command::{dir, download::wget, CommandExt, Ext, Tar},
    Arch,
};
use std::{
    env,
    ffi::OsString,
    fs,
    os::unix,
    path::{Path, PathBuf},
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

    /// 将 musl 动态库放入 rootfs。
    pub fn put_musl_libs(&self) -> PathBuf {
        // 递归 rootfs
        self.make(false);
        let dir = self.0.linux_musl_cross();
        self.put_libs(&dir, dir.join(format!("{}-linux-musl", self.0.name())));
        dir
    }

    /// 指定架构的 rootfs 路径。
    #[inline]
    fn path(&self) -> PathBuf {
        PathBuf::from_iter(["rootfs", self.0.name()])
    }

    /// 下载并解压 minirootfs。
    fn prebuild_rootfs(&self) -> PathBuf {
        // 构造压缩文件路径
        let tar = self.0.origin().join(match self.0 {
            Arch::Riscv64 => "minirootfs.tar.xz",
            Arch::X86_64 | Arch::Aarch64 => "minirootfs.tar.gz",
        });
        // 构造下载地址
        let url = match self.0 {
            Arch::Riscv64 =>
                String::from("https://github.com/rcore-os/libc-test-prebuilt/releases/download/0.1/prebuild.tar.xz"),
            Arch::X86_64 | Arch::Aarch64 =>
                // 只能使用 3.12 这个版本
                format!("https://dl-cdn.alpinelinux.org/alpine/v3.12/releases/{arch}/alpine-minirootfs-3.12.0-{arch}.tar.gz", arch = self.0.name()),
        };
        // 下载
        wget(url, &tar);
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

    /// 从安装目录拷贝所有 so 和 so 链接到 rootfs
    fn put_libs(&self, musl: impl AsRef<Path>, dir: impl AsRef<Path>) {
        let lib = self.path().join("lib");
        let strip = musl
            .as_ref()
            .join("bin")
            .join(format!("{}-linux-musl-strip", self.0.name()));
        dir.as_ref()
            .join("lib")
            .read_dir()
            .unwrap()
            .filter_map(|res| res.map(|e| e.path()).ok())
            .filter(|path| check_so(path))
            .for_each(|source| {
                let target = lib.join(source.file_name().unwrap());
                dir::rm(&target).unwrap();
                if source.is_symlink() {
                    // `fs::copy` 会拷贝文件内容
                    unix::fs::symlink(source.read_link().unwrap(), target).unwrap();
                } else {
                    fs::copy(source, &target).unwrap();
                    Ext::new(&strip).arg("-s").arg(target).status();
                }
            });
    }
}

/// 为 PATH 环境变量附加路径。
fn join_path_env<I, S>(paths: I) -> OsString
where
    I: IntoIterator<Item = S>,
    S: AsRef<Path>,
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
        path.push(item.as_ref().canonicalize().unwrap().as_os_str());
    }
    path
}

fn check_so<P: AsRef<Path>>(path: P) -> bool {
    let path = path.as_ref();
    // 是符号链接或文件
    // 对于符号链接，`is_file` `exist` 等函数都会针对其指向的真实文件判断
    if !path.is_symlink() && !path.is_file() {
        return false;
    }
    let name = path.file_name().unwrap().to_string_lossy();
    let mut seg = name.split('.');
    // 不能以 . 开头
    if matches!(seg.next(), Some("") | None) {
        return false;
    }
    // 扩展名的第一项是 so
    if !matches!(seg.next(), Some("so")) {
        return false;
    }
    // so 之后全是纯十进制数字
    !seg.any(|it| !it.chars().all(|ch| ch.is_ascii_digit()))
}

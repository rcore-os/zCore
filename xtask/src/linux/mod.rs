use crate::{
    command::{dir, download::wget, CommandExt, Ext, Make, Qemu, Tar},
    Arch, ALPINE_ROOTFS_VERSION, ALPINE_WEBSITE,
};
use dircpy::copy_dir;
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix,
    path::{Path, PathBuf},
};

pub(crate) struct LinuxRootfs(Arch);

impl LinuxRootfs {
    #[inline]
    pub fn new(arch: Arch) -> Self {
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
            Arch::X86_64 => libos_libc_so(),
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

    /// 将 libc-test 放入 rootfs。
    pub fn put_libc_test(&self) {
        // 递归 rootfs
        self.make(false);
        // 拷贝仓库
        let dir = self.libc_test();
        dir::rm(&dir).unwrap();
        copy_dir("libc-test", &dir).unwrap();
        // 编译
        fs::copy(dir.join("config.mak.def"), dir.join("config.mak")).unwrap();
        match self.0 {
            Arch::Riscv64 => {
                Make::new(None)
                    .env("ARCH", self.0.name())
                    .env("CROSS_COMPILE", "riscv64-linux-musl-")
                    .env("PATH", join_path(&[linux_musl_cross(self.0)]))
                    .current_dir(&dir)
                    .invoke();
                fs::copy(
                    self.0
                        .target()
                        .join("rootfs/libc-test/functional/tls_align-static.exe"),
                    dir.join("src/functional/tls_align-static.exe"),
                )
                .unwrap();
            }
            Arch::X86_64 => {
                fs::OpenOptions::new()
                    .append(true)
                    .open(dir.join("config.mak"))
                    .unwrap()
                    .write_all(b"CC := musl-gcc\nAR := ar\nRANLIB := ranlib")
                    .unwrap();
                Make::new(None).current_dir(dir).invoke();
            }
            Arch::Aarch64 => {
                Make::new(None)
                    .env("ARCH", self.0.name())
                    .env("CROSS_COMPILE", "aarch64-linux-musl-")
                    .env("PATH", join_path(&[linux_musl_cross(Arch::Aarch64)]))
                    .current_dir(&dir)
                    .invoke();
            }
        }
    }

    /// 将其他测试放入 rootfs。
    pub fn put_other_test(&self) {
        // 递归 rootfs
        self.make(false);
        let rootfs = self.path();
        match self.0 {
            Arch::Riscv64 => {
                copy_dir(self.0.target().join("rootfs/oscomp"), rootfs.join("oscomp")).unwrap();
            }
            Arch::X86_64 => {
                let bin = rootfs.join("bin");
                fs::read_dir("linux-syscall/test")
                    .unwrap()
                    .filter_map(|res| res.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().map_or(false, |ext| ext == OsStr::new("c")))
                    .for_each(|c| {
                        Ext::new("gcc")
                            .arg(&c)
                            .arg("-o")
                            .arg(bin.join(c.file_prefix().unwrap()))
                            .arg("-Wl,--dynamic-linker=/lib/ld-musl-x86_64.so.1")
                            .invoke();
                    });
            }
            Arch::Aarch64 => {
                let musl_cross = linux_musl_cross(self.0);
                let bin = rootfs.join("bin");
                fs::read_dir("linux-syscall/test")
                    .unwrap()
                    .filter_map(|res| res.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().map_or(false, |ext| ext == OsStr::new("c")))
                    .for_each(|c| {
                        Ext::new(musl_cross.join("aarch64-linux-musl-gcc"))
                            .arg(&c)
                            .arg("-o")
                            .arg(bin.join(c.file_prefix().unwrap()))
                            .invoke();
                    });
            }
        }
    }

    /// 生成镜像。
    pub fn image(&self) {
        // 递归 rootfs
        self.make(false);
        let arch_str = self.0.name();
        let image = match self.0 {
            Arch::Riscv64 | Arch::Aarch64 => {
                let rootfs = format!("rootfs/{arch_str}");
                let image = format!("zCore/{arch_str}.img");
                fuse(rootfs, &image);
                image
            }
            Arch::X86_64 => {
                let rootfs = self.path();
                let to = rootfs.join("lib/ld-musl-x86_64.so.1");
                fs::copy(self.0.target().join("rootfs/lib/ld-musl-x86_64.so.1"), &to).unwrap();

                let image = format!("zCore/{arch_str}.img");
                fuse(rootfs, &image);

                fs::copy(libos_libc_so(), to).unwrap();

                image
            }
        };
        Qemu::img()
            .arg("resize")
            .args(&["-f", "raw"])
            .arg(image)
            .arg("+5M")
            .invoke();
    }

    #[inline]
    fn path(&self) -> PathBuf {
        PathBuf::from_iter(["rootfs", self.0.name()])
    }

    #[inline]
    fn libc_test(&self) -> PathBuf {
        self.path().join("libc-test")
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

/// 下载适用于 libos 的 musl libc so。
fn libos_libc_so() -> PathBuf {
    const NAME: &str = "libc-libos.so";

    let url =
        format!("https://github.com/rcore-os/libc-test-prebuilt/releases/download/master/{NAME}");
    let dst = Arch::X86_64.origin().join(NAME);
    wget(url, &dst);
    dst
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
fn join_path<I, S>(paths: I) -> OsString
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

/// 制作镜像。
fn fuse(dir: impl AsRef<Path>, image: impl AsRef<Path>) {
    use rcore_fs::vfs::FileSystem;
    use rcore_fs_fuse::zip::zip_dir;
    use rcore_fs_sfs::SimpleFileSystem;
    use std::sync::{Arc, Mutex};

    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(image)
        .expect("failed to open image");
    const MAX_SPACE: usize = 1024 * 1024 * 1024; // 1GiB
    let fs = SimpleFileSystem::create(Arc::new(Mutex::new(file)), MAX_SPACE)
        .expect("failed to create sfs");
    zip_dir(dir.as_ref(), fs.root_inode()).expect("failed to zip fs");
}

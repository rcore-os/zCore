//! 平台相关的操作。

use crate::{dir, download::wget, CommandExt, ALPINE_ROOTFS_VERSION, ALPINE_WEBSITE};
use dircpy::copy_dir;
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix,
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

impl ArchCommands {
    const fn as_str(&self) -> &'static str {
        match self {
            ArchCommands::Riscv64 => "riscv64",
            ArchCommands::X86_64 => "x86_64",
        }
    }

    fn rootfs(&self) -> PathBuf {
        let mut path = PathBuf::new();
        path.push("rootfs");
        path.push(self.as_str());
        path
    }

    fn libc_test(&self) -> PathBuf {
        let mut path = self.rootfs();
        path.push("libc-test");
        path
    }

    fn origin(&self) -> PathBuf {
        let mut path = PathBuf::new();
        path.push("ignored");
        path.push("origin");
        path.push(self.as_str());
        path
    }

    fn target(&self) -> PathBuf {
        let mut path = PathBuf::new();
        path.push("ignored");
        path.push("target");
        path.push(self.as_str());
        path
    }
}

impl Arch {
    /// 构造启动内存文件系统 rootfs。
    /// 对于 x86_64，这个文件系统可用于 libos 启动。
    /// 若设置 `clear`，将清除已存在的目录。
    pub fn rootfs(&self, clear: bool) {
        // 若已存在且不需要清空，可以直接退出
        let dir = self.command.rootfs();
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
        let libc_so = format!("lib/ld-musl-{arch}.so.1", arch = self.command.as_str());
        let so = match self.command {
            ArchCommands::Riscv64 => src.join(&libc_so),
            ArchCommands::X86_64 => PathBuf::from("prebuilt/linux/libc-libos.so"),
        };
        fs::copy(so, dir.join(libc_so)).unwrap();
    }

    /// 将 libc-test 放入 rootfs。
    pub fn libc_test(&self) {
        // 递归 rootfs
        self.rootfs(false);
        // 拷贝仓库
        let dir = self.command.libc_test();
        dir::rm(&dir).unwrap();
        copy_dir("libc-test", &dir).unwrap();
        // 编译
        fs::copy(dir.join("config.mak.def"), dir.join("config.mak")).unwrap();
        match self.command {
            ArchCommands::Riscv64 => {
                Make::new(None)
                    .env("ARCH", self.command.as_str())
                    .env("CROSS_COMPILE", "riscv64-linux-musl-")
                    .env("PATH", riscv64_linux_musl_cross())
                    .current_dir(&dir)
                    .invoke();
                fs::copy(
                    self.command
                        .target()
                        .join("rootfs/libc-test/functional/tls_align-static.exe"),
                    dir.join("src/functional/tls_align-static.exe"),
                )
                .unwrap();
            }
            ArchCommands::X86_64 => {
                fs::OpenOptions::new()
                    .append(true)
                    .open(dir.join("config.mak"))
                    .unwrap()
                    .write_all(b"CC := musl-gcc\nAR := ar\nRANLIB := ranlib")
                    .unwrap();
                Make::new(None).current_dir(dir).invoke();
            }
        }
    }

    /// 将其他测试放入 rootfs。
    pub fn other_test(&self) {
        // 递归 rootfs
        self.rootfs(false);
        let rootfs = self.command.rootfs();
        unix::fs::symlink("busybox", rootfs.join("bin/ls")).unwrap();
        match self.command {
            ArchCommands::Riscv64 => {
                let target = self.command.target();
                copy_dir(target.join("rootfs/oscomp"), rootfs.join("oscomp")).unwrap();
            }
            ArchCommands::X86_64 => {
                let bin = rootfs.join("bin");
                fs::read_dir("linux-syscall/test")
                    .unwrap()
                    .filter_map(|res| res.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().map_or(false, |ext| ext == OsStr::new("c")))
                    .for_each(|c| {
                        Command::new("gcc")
                            .arg(&c)
                            .arg("-o")
                            .arg(bin.join(c.file_prefix().unwrap()))
                            .arg("-Wl,--dynamic-linker=/lib/ld-musl-x86_64.so.1")
                            .status()
                            .unwrap()
                            .exit_ok()
                            .expect("FAILED: gcc {c:?}");
                    });
            }
        }
    }

    /// 生成镜像。
    pub fn image(&self) {
        // 递归 rootfs
        self.rootfs(false);
        let arch_str = self.command.as_str();
        let image = match self.command {
            ArchCommands::Riscv64 => {
                let rootfs = format!("rootfs/{arch_str}");
                let image = format!("zCore/{arch_str}.img");
                fuse(rootfs, &image);
                image
            }
            ArchCommands::X86_64 => {
                let target = self.command.target();
                let rootfs = self.command.rootfs();
                fs::copy(
                    target.join("rootfs/lib/ld-musl-x86_64.so.1"),
                    rootfs.join("lib/ld-musl-x86_64.so.1"),
                )
                .unwrap();

                let image = format!("zCore/{arch_str}.img");
                fuse(&rootfs, &image);

                fs::copy(
                    "prebuilt/linux/libc-libos.so",
                    rootfs.join("lib/ld-musl-x86_64.so.1"),
                )
                .unwrap();

                image
            }
        };
        Command::new("qemu-img")
            .args(&["resize", &image, "+5M"])
            .status()
            .unwrap()
            .exit_ok()
            .expect("FAILED: qemu-img resize");
    }

    /// 下载并解压 minirootfs。
    fn prebuild_rootfs(&self) -> PathBuf {
        // 构造压缩文件路径
        let file_name = match self.command {
            ArchCommands::Riscv64 => "minirootfs.tar.xz",
            ArchCommands::X86_64 => "minirootfs.tar.gz",
        };
        let tar = self.command.origin().join(file_name);
        // 若压缩文件不存在，需要下载
        if !tar.exists() {
            let url = match self.command {
                ArchCommands::Riscv64 => String::from("https://github.com/rcore-os/libc-test-prebuilt/releases/download/0.1/prebuild.tar.xz"),
                ArchCommands::X86_64 => format!("{ALPINE_WEBSITE}/x86_64/alpine-minirootfs-{ALPINE_ROOTFS_VERSION}-x86_64.tar.gz"),
            };
            wget(url, &tar);
        }
        // 解压到目标路径
        let dir = self.command.target().join("rootfs");
        dir::clear(&dir).unwrap();
        let mut tar = Tar::xf(&tar, Some(&dir));
        match self.command {
            ArchCommands::Riscv64 => tar.args(&["--strip-components", "1"]).invoke(),
            ArchCommands::X86_64 => tar.invoke(),
        }
        dir
    }
}

struct Make(Command);

impl AsRef<Command> for Make {
    fn as_ref(&self) -> &Command {
        &self.0
    }
}

impl AsMut<Command> for Make {
    fn as_mut(&mut self) -> &mut Command {
        &mut self.0
    }
}

impl CommandExt for Make {}

impl Make {
    fn new(j: Option<usize>) -> Self {
        let mut make = Self(Command::new("make"));
        match j {
            Some(0) => {}
            Some(j) => {
                make.arg(format!("-j{j}"));
            }
            None => {
                make.arg("-j");
            }
        }
        make
    }
}

struct Tar(Command);

impl AsRef<Command> for Tar {
    fn as_ref(&self) -> &Command {
        &self.0
    }
}

impl AsMut<Command> for Tar {
    fn as_mut(&mut self) -> &mut Command {
        &mut self.0
    }
}

impl CommandExt for Tar {}

impl Tar {
    fn xf(src: &impl AsRef<OsStr>, dst: Option<impl AsRef<OsStr>>) -> Self {
        let mut cmd = Command::new("tar");
        cmd.arg("xf").arg(src);
        if let Some(dst) = dst {
            cmd.arg("-C").arg(dst);
        }
        Self(cmd)
    }
}

/// 下载 riscv64-musl 工具链。
fn riscv64_linux_musl_cross() -> OsString {
    const NAME: &str = "riscv64-linux-musl-cross";

    let origin = ArchCommands::Riscv64.origin();
    let target = ArchCommands::Riscv64.target();

    let tgz = origin.join(format!("{NAME}.tgz"));
    let dir = target.join(NAME);

    dir::rm(&dir).unwrap();
    wget(&format!("https://musl.cc/{NAME}.tgz"), &tgz);
    Tar::xf(&tgz, Some(target)).invoke();

    // 将交叉工具链加入 PATH 环境变量
    let mut path = OsString::new();
    if let Ok(current) = std::env::var("PATH") {
        path.push(current);
        path.push(":");
    }
    path.push(std::env::current_dir().unwrap());
    path.push("/");
    path.push(dir);
    path.push("/bin");
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
    const MAX_SPACE: usize = 0x1000 * 0x1000 * 1024; // 1G
    let fs = SimpleFileSystem::create(Arc::new(Mutex::new(file)), MAX_SPACE)
        .expect("failed to create sfs");
    zip_dir(dir.as_ref(), fs.root_inode()).expect("failed to zip fs");
}

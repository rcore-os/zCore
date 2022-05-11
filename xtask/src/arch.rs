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

    fn rootfs(&self) -> String {
        format!("rootfs/{}", self.as_str())
    }

    fn ignored(&self) -> String {
        format!("ignored/{}", self.as_str())
    }
}

impl Arch {
    /// 构造启动内存文件系统 rootfs。
    ///
    /// 若 `clear` == true，将清除已存在的目录。
    ///
    /// 将在文件系统中放置必要的库文件。
    pub fn rootfs(&self, clear: bool) {
        let dir = PathBuf::from(self.command.rootfs());
        if dir.is_dir() && !clear {
            return;
        }

        self.wget_alpine();
        let arch_str = self.command.as_str();
        let tar = dir::detect(self.command.ignored(), "minirootfs").unwrap();
        match self.command {
            ArchCommands::Riscv64 => {
                dir::clear(&dir).unwrap();
                Tar::xf(&tar, Some(&dir))
                    .args(&["--strip-components", "1"])
                    .join();
                let mut ls = dir;
                ls.push("bin");
                ls.push("ls");
                unix::fs::symlink("busybox", ls).unwrap();
            }
            ArchCommands::X86_64 => {
                {
                    dir::clear(&dir).unwrap();
                    Tar::xf(&tar, Some(&dir)).join();
                    // libc-libos.so (convert syscall to function call) is from https://github.com/rcore-os/musl/tree/rcore
                    fs::copy(
                        "prebuilt/linux/libc-libos.so",
                        dir.join("lib").join(format!("ld-musl-{arch_str}.so.1")),
                    )
                    .unwrap();
                }
                {
                    const TEST_DIR: &str = "linux-syscall/test";
                    let mut dest_dir = dir;
                    dest_dir.push("bin");
                    // for linux syscall tests
                    fs::read_dir(TEST_DIR)
                        .unwrap()
                        .filter_map(|res| res.ok())
                        .map(|entry| entry.path())
                        .filter(|path| path.extension().map_or(false, |ext| ext == OsStr::new("c")))
                        .for_each(|c| {
                            let mut o = dest_dir.clone();
                            o.push(c.file_prefix().and_then(|s| s.to_str()).unwrap());
                            Command::new("gcc")
                                .arg(&c)
                                .arg("-o")
                                .arg(&o)
                                .arg("-Wl,--dynamic-linker=/lib/ld-musl-x86_64.so.1")
                                .status()
                                .unwrap()
                                .exit_ok()
                                .expect("FAILED: gcc {c:?}");
                        });
                }
            }
        }
    }

    /// 将 libc-test 放入 rootfs。
    pub fn libc_test(&self) {
        self.rootfs(false);

        let arch_str = self.command.as_str();
        let rootfs = self.command.rootfs();
        let dir = format!("{rootfs}/libc-test");
        match self.command {
            ArchCommands::Riscv64 => {
                let pre = format!("{rootfs}/libc-test-prebuilt");

                fs::rename(&dir, &pre).unwrap();
                copy_dir("libc-test", &dir).unwrap();
                fs::copy(format!("{dir}/config.mak.def"), format!("{dir}/config.mak")).unwrap();
                Make::new(None)
                    .env("ARCH", arch_str)
                    .env("CROSS_COMPILE", "riscv64-linux-musl-")
                    .env("PATH", riscv64_linux_musl_cross())
                    .current_dir(&dir)
                    .join();
                fs::copy(
                    format!("{pre}/functional/tls_align-static.exe"),
                    format!("{dir}/src/functional/tls_align-static.exe"),
                )
                .unwrap();
                dir::rm(pre).unwrap();
            }
            ArchCommands::X86_64 => {
                dir::rm(&dir).unwrap();
                copy_dir("libc-test", &dir).unwrap();
                fs::copy(format!("{dir}/config.mak.def"), format!("{dir}/config.mak")).unwrap();
                fs::OpenOptions::new()
                    .append(true)
                    .open(format!("{dir}/config.mak"))
                    .unwrap()
                    .write_all(b"CC := musl-gcc\nAR := ar\nRANLIB := ranlib")
                    .unwrap();
                Make::new(None).current_dir(dir).join();
            }
        }
    }

    /// 生成镜像。
    pub fn image(&self) {
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
                let tmp: usize = rand::random();
                let tmp_rootfs = format!("/tmp/{tmp}");

                let rootfs = format!("rootfs/{arch_str}");
                let rootfs_lib = format!("{rootfs}/lib");

                // ld-musl-x86_64.so.1 替换为适用 bare-matel 的版本
                dir::clear(&tmp_rootfs).unwrap();
                dir::clear(&rootfs_lib).unwrap();

                let tar = dir::detect(ArchCommands::X86_64.ignored(), "minirootfs").unwrap();
                Tar::xf(&tar, Some(&tmp_rootfs)).join();

                fs::copy(
                    format!("{tmp_rootfs}/lib/ld-musl-x86_64.so.1"),
                    format!("{rootfs_lib}/ld-musl-x86_64.so.1"),
                )
                .unwrap();
                dir::rm(tmp_rootfs).unwrap();

                let image = format!("zCore/{arch_str}.img");
                fuse(rootfs, &image);

                fs::copy(
                    "prebuilt/linux/libc-libos.so",
                    format!("{rootfs_lib}/ld-musl-x86_64.so.1"),
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

    /// 获取 alpine 镜像。
    fn wget_alpine(&self) {
        let arch_str = self.command.as_str();
        let ignored = self.command.ignored();
        let (local_path, web_url) = match self.command {
            ArchCommands::Riscv64 => {
                const FILE_NAME: &str = "minirootfs.tar.xz";
                const WEB_URL: &str = "https://github.com/rcore-os/libc-test-prebuilt/releases/download/0.1/prebuild.tar.xz";

                let local_path = PathBuf::from(format!("{ignored}/{FILE_NAME}"));
                if local_path.exists() {
                    return;
                }
                (local_path, WEB_URL.into())
            }
            ArchCommands::X86_64 => {
                const FILE_NAME: &str = "minirootfs.tar.gz";

                let local_path = PathBuf::from(format!("{ignored}/{FILE_NAME}"));
                if local_path.exists() {
                    return;
                }
                (
                    local_path,
                    format!(
                        "{ALPINE_WEBSITE}/{arch_str}/alpine-minirootfs-{ALPINE_ROOTFS_VERSION}-{arch_str}.tar.gz"
                    ),
                )
            }
        };

        fs::create_dir_all(local_path.parent().unwrap()).unwrap();
        wget(&web_url, &local_path);
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
    const DIR: &str = "ignored/riscv64";
    const NAME: &str = "riscv64-linux-musl-cross";
    let dir = format!("{DIR}/{NAME}");
    let tgz = format!("{dir}.tgz");

    wget(&format!("https://musl.cc/{NAME}.tgz"), &tgz);
    dir::rm(&dir).unwrap();
    Tar::xf(&tgz, Some(DIR)).join();

    // 将交叉工具链加入 PATH 环境变量
    let mut path = OsString::new();
    if let Ok(current) = std::env::var("PATH") {
        path.push(current);
        path.push(":");
    }
    path.push(std::env::current_dir().unwrap());
    path.push(dir);
    path.push("bin");
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

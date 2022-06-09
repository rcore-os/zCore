use crate::{
    command::{dir, download::wget, CommandExt, Ext, Make, Tar},
    Arch,
};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::PathBuf,
};

impl super::LinuxRootfs {
    /// 将 libc-test 放入 rootfs。
    pub fn put_libc_test(&self) {
        // 递归 rootfs
        self.make(false);
        // 拷贝仓库
        let dir = self.path().join("libc-test");
        dir::rm(&dir).unwrap();
        dircpy::copy_dir("libc-test", &dir).unwrap();
        // 编译
        fs::copy(dir.join("config.mak.def"), dir.join("config.mak")).unwrap();
        match self.0 {
            Arch::Riscv64 => {
                Make::new(None)
                    .env("ARCH", self.0.name())
                    .env("CROSS_COMPILE", "riscv64-linux-musl-")
                    .env("PATH", join_path_env(&[linux_musl_cross(self.0)]))
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
                    .env("PATH", join_path_env(&[linux_musl_cross(Arch::Aarch64)]))
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
                dircpy::copy_dir(self.0.target().join("rootfs/oscomp"), rootfs.join("oscomp"))
                    .unwrap();
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

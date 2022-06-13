use super::{join_path_env, linux_musl_cross};
use crate::{
    command::{dir, CommandExt, Ext, Make},
    Arch,
};
use std::{ffi::OsStr, fs, io::Write};

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
                Make::new()
                    .j(usize::MAX)
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
                Make::new().j(usize::MAX).current_dir(dir).invoke();
            }
            Arch::Aarch64 => {
                Make::new()
                    .j(usize::MAX)
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

use super::{join_path_env, linux_musl_cross};
use crate::{
    command::{download::git_clone, CommandExt, Ext, Make},
    Arch, ORIGIN,
};
use std::{fs, path::PathBuf};

impl super::LinuxRootfs {
    pub fn put_opencv(&self) {
        // 递归 rootfs
        self.make(false);
        // 拉 opencv
        let opencv = PathBuf::from(ORIGIN).join("opencv");
        git_clone("https://github.com/opencv/opencv.git", &opencv);
        // 构建
        let opencv = opencv.canonicalize().unwrap();
        let build = self.0.target().join("opencv");
        match self.0 {
            Arch::Riscv64 => {
                let path_with_musl_gcc = join_path_env(&[linux_musl_cross(self.0)]);
                let platform_cmake =
                    PathBuf::from("xtask/src/linux/riscv64-musl-gcc.toolchain.cmake")
                        .canonicalize()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                fs::create_dir_all(&build).unwrap();
                Ext::new("cmake")
                    .current_dir(&build)
                    .arg(format!("-DCMAKE_TOOLCHAIN_FILE={platform_cmake}"))
                    .arg("-DCMAKE_INSTALL_PREFIX=install")
                    .arg(opencv)
                    .env("PATH", &path_with_musl_gcc)
                    .invoke();
                Make::install()
                    .current_dir(&build)
                    .j(num_cpus::get().min(8)) // 不能用太多线程，以免爆内存
                    .env("PATH", path_with_musl_gcc)
                    .invoke();
            }
            Arch::X86_64 | Arch::Aarch64 => todo!(),
        }
        // 拷贝
        let lib = self.path().join("lib");
        build
            .join("install")
            .join("lib")
            .read_dir()
            .unwrap()
            .filter_map(|res| res.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_file() || path.is_symlink())
            .for_each(|so| {
                let to = lib.join(so.file_name().unwrap());
                fs::copy(so, to).unwrap();
            });
    }
}

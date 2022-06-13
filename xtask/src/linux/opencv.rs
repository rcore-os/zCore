use super::{join_path_env, linux_musl_cross};
use crate::{
    command::{dir, download::fetch_online, CommandExt, Ext, Git, Make},
    Arch, ORIGIN,
};
use std::{
    fs,
    os::unix,
    path::{Path, PathBuf},
};

impl super::LinuxRootfs {
    pub fn put_opencv(&self) {
        // 递归 rootfs
        self.make(false);
        // 拉 opencv
        let opencv = PathBuf::from(ORIGIN).join("opencv");
        if !opencv.is_dir() {
            fetch_online!(opencv, |tmp| {
                Git::clone("https://github.com/opencv/opencv.git")
                    .dir(tmp)
                    .single_branch()
                    .depth(1)
                    .done()
            });
        }
        let source = opencv.canonicalize().unwrap();
        let target = self.0.target().join("opencv");
        // 如果 build 目录不存在，需要执行 cmake
        let cmake_needed = !target.is_dir();
        // 如果执行了 cmake 或安装目录不存在，需要 make
        let install_needed = cmake_needed || !target.join("install").is_dir();
        // 工具链
        let path_with_musl_gcc = join_path_env(&[linux_musl_cross(self.0)]);
        //
        if cmake_needed {
            dir::clear(&target).unwrap();
            // ffmpeg 路径
            let ffmpeg = PathBuf::from(ORIGIN)
                .join("ffmpeg")
                .join("install")
                .join("lib");
            // 创建平台相关 cmake
            let platform_cmake = self.0.target().join("musl-gcc.toolchain.cmake");
            fs::write(&platform_cmake, self.opencv_cmake(&ffmpeg)).unwrap();
            // 执行
            let mut cmake = Ext::new("cmake");
            if ffmpeg.is_dir() {
                cmake.env(
                    "PKG_CONFIG_LIBDIR",
                    ffmpeg.join("pkgconfig").canonicalize().unwrap(),
                );
            }
            cmake
                .current_dir(&target)
                .arg(format!(
                    "-DCMAKE_TOOLCHAIN_FILE={}",
                    platform_cmake.canonicalize().unwrap().display()
                ))
                .arg("-DWITH_FFMPEG=ON")
                .arg("-DCMAKE_BUILD_TYPE=Release")
                .arg(format!(
                    "-DCMAKE_INSTALL_PREFIX={}",
                    target.canonicalize().unwrap().join("install").display(),
                ))
                .arg(source)
                .env("PATH", &path_with_musl_gcc)
                .invoke();
        }
        //
        if install_needed {
            Make::install()
                .current_dir(&target)
                .j(num_cpus::get().min(8)) // 不能用太多线程，以免爆内存
                .env("PATH", path_with_musl_gcc)
                .invoke();
        }
        // 拷贝
        self.put_libs(target);
    }

    pub fn put_ffmpeg(&self) {
        // 递归 rootfs
        self.make(false);
        // 拉 ffmpeg
        let ffmpeg = PathBuf::from(ORIGIN).join("ffmpeg");
        if !ffmpeg.is_dir() {
            fetch_online!(ffmpeg, |tmp| {
                Git::clone("https://github.com/FFmpeg/FFmpeg.git")
                    .dir(tmp)
                    .branch("release/5.0")
                    .single_branch()
                    .depth(1)
                    .done()
            });
        }
        // 构建
        match self.0 {
            Arch::Riscv64 => {
                let path_with_musl_gcc = join_path_env(&[linux_musl_cross(self.0)]);
                println!("Configuring ffmpeg, please wait...");
                Ext::new("./configure")
                    .current_dir(&ffmpeg)
                    .arg("--enable-cross-compile")
                    .arg("--cross-prefix=riscv64-linux-musl-")
                    .arg("--arch=riscv64")
                    .arg("--target-os=linux")
                    .arg("--enable-static")
                    .arg("--enable-shared")
                    .arg("--disable-doc")
                    .arg(format!(
                        "--prefix={}",
                        ffmpeg.canonicalize().unwrap().join("install").display(),
                    ))
                    .env("PATH", &path_with_musl_gcc)
                    .invoke();
                Make::install()
                    .current_dir(&ffmpeg)
                    .j(num_cpus::get().min(8)) // 不能用太多线程，以免爆内存
                    .env("PATH", path_with_musl_gcc)
                    .invoke();
            }
            Arch::X86_64 | Arch::Aarch64 => todo!(),
        }
        // 拷贝
        self.put_libs(ffmpeg);
    }

    /// 从安装目录拷贝所有 so 和 so 链接到 rootfs
    fn put_libs(&self, build: impl AsRef<Path>) {
        let lib = self.path().join("lib");
        build
            .as_ref()
            .join("install")
            .join("lib")
            .read_dir()
            .unwrap()
            .filter_map(|res| res.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file() && path.file_name().unwrap().to_string_lossy().contains(".so")
            })
            .for_each(|source| {
                let target = lib.join(source.file_name().unwrap());
                dir::rm(&target).unwrap();
                if source.is_symlink() {
                    // `fs::copy` 会拷贝文件内容
                    unix::fs::symlink(source.read_link().unwrap(), target).unwrap();
                } else {
                    fs::copy(source, target).unwrap();
                }
            });
    }

    /// 构造一个用于 opencv 构建的 cmake 文件。
    fn opencv_cmake(&self, ffmpeg: impl AsRef<Path>) -> String {
        // 不会写 cmake
        if !matches!(self.0, Arch::Riscv64) {
            todo!();
        }
        const HEAD: &str = "\
set(CMAKE_SYSTEM_NAME      \"Linux\")
set(CMAKE_SYSTEM_PROCESSOR \"riscv64\")

set(CMAKE_C_COMPILER   riscv64-linux-musl-gcc)
set(CMAKE_CXX_COMPILER riscv64-linux-musl-g++)

set(CMAKE_C_FLAGS   \"\" CACHE STRING \"\")
set(CMAKE_CXX_FLAGS \"\" CACHE STRING \"\")

set(CMAKE_C_FLAGS   \"-march=rv64gc ${CMAKE_C_FLAGS}   ${CMAKE_PASS_TEST_FLAGS}\")
set(CMAKE_CXX_FLAGS \"-march=rv64gc ${CMAKE_CXX_FLAGS} ${CMAKE_PASS_TEST_FLAGS}\")";

        let ffmpeg = ffmpeg.as_ref();
        if ffmpeg.is_dir() {
            format!(
                "\
{HEAD}

set(CMAKE_LD_FFMPEG_FLAGS  \"-Wl,-rpath-link,{}\")
set(CMAKE_EXE_LINKER_FLAGS \"${{CMAKE_EXE_LINKER_FLAGS}} ${{CMAKE_LD_FFMPEG_FLAGS}}\")",
                ffmpeg.canonicalize().unwrap().display()
            )
        } else {
            HEAD.into()
        }
    }
}

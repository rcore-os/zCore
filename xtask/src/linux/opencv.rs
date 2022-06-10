use super::{join_path_env, linux_musl_cross};
use crate::{
    command::{download::fetch_online, CommandExt, Ext, Git, Make},
    Arch, ORIGIN,
};
use std::{
    fs,
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
                    .done()
            });
        }
        // 构建
        let opencv = opencv.canonicalize().unwrap();
        let build = self.0.target().join("opencv");
        match self.0 {
            Arch::Riscv64 => {
                // 工具链路径放入 PATH
                let path_with_musl_gcc = join_path_env(&[linux_musl_cross(self.0)]);
                // ffmpeg 路径
                let ffmpeg = PathBuf::from(ORIGIN)
                    .join("ffmpeg")
                    .join("install")
                    .join("lib");
                // 创建平台相关 cmake
                let platform_cmake = self.0.target().join("riscv64-musl-gcc.toolchain.cmake");
                fs::write(&platform_cmake, riscv64_opencv_cmake(&ffmpeg)).unwrap();
                // 创建生成目录
                fs::create_dir_all(&build).unwrap();
                let mut cmake = Ext::new("cmake");
                if ffmpeg.is_dir() {
                    cmake.env(
                        "PKG_CONFIG_LIBDIR",
                        ffmpeg.join("pkgconfig").canonicalize().unwrap(),
                    );
                }
                cmake
                    .current_dir(&build)
                    .arg(format!(
                        "-DCMAKE_TOOLCHAIN_FILE={}",
                        platform_cmake.canonicalize().unwrap().display()
                    ))
                    .arg("-DWITH_FFMPEG=ON")
                    .arg("-DCMAKE_BUILD_TYPE=Release")
                    .arg(format!(
                        "-DCMAKE_INSTALL_PREFIX={}",
                        build.canonicalize().unwrap().join("install").display(),
                    ))
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
        self.put_libs(build);
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
                    .done()
            });
        }
        // 构建
        match self.0 {
            Arch::Riscv64 => {
                let path_with_musl_gcc = join_path_env(&[linux_musl_cross(self.0)]);
                println!("Configuring ffmpeg, please waiting...");
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
                (path.is_file() || path.is_symlink())
                    && path.file_name().unwrap().to_string_lossy().contains(".so")
            })
            .for_each(|so| {
                let to = lib.join(so.file_name().unwrap());
                fs::copy(so, to).unwrap();
            });
    }
}

fn riscv64_opencv_cmake(ffmpeg: impl AsRef<Path>) -> String {
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

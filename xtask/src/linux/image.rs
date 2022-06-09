use super::{LinuxRootfs, LIBOS_MUSL_LIBC_PATH};
use crate::{
    command::{CommandExt, Qemu},
    Arch,
};
use std::{fs,  path::Path};

impl LinuxRootfs {
    /// 生成镜像。
    pub fn image(&self) {
        // 递归 rootfs
        self.make(false);
        // 镜像路径
        let image = format!("zCore/{arch}.img", arch = self.0.name());
        // 生成镜像
        match self.0 {
            Arch::Riscv64 | Arch::Aarch64 => fuse(self.path(), &image),
            Arch::X86_64 => {
                let rootfs = self.path();
                let to = rootfs.join("lib/ld-musl-x86_64.so.1");

                // 拷贝适用于真机的 musl_libc.so
                // 这个文件在构造 rootfs 过程中必然已经产生了
                fs::copy(self.0.target().join("rootfs/lib/ld-musl-x86_64.so.1"), &to).unwrap();

                // 生成镜像
                fuse(rootfs, &image);

                // 恢复适用于 libos 的 musl_libc.so
                fs::copy(LIBOS_MUSL_LIBC_PATH.as_path(), to).unwrap();
            }
        }
        // 扩充一些额外空间，供某些测试使用
        Qemu::img()
            .arg("resize")
            .args(&["-f", "raw"])
            .arg(image)
            .arg("+5M")
            .invoke();
    }
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

use super::{LinuxRootfs, LIBOS_MUSL_LIBC_PATH};
use crate::{
    command::{dir, download::wget, CommandExt, Ext, Qemu, Tar},
    Arch,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

impl LinuxRootfs {
    /// 生成镜像。
    pub fn image(&self) {
        // 递归 rootfs
        self.make(false);
        // 镜像路径
        let image = format!("zCore/{arch}.img", arch = self.0.name());
        // aarch64 还需要下载 firmware
        if let Arch::Aarch64 = self.0 {
            const URL:&str = "https://github.com/Luchangcheng2333/rayboot/releases/download/2.0.0/aarch64_firmware.tar.gz";
            let aarch64_tar = self.0.origin().join("Aarch64_firmware.zip");
            wget(URL, &aarch64_tar);

            let fw_dir = self.0.target().join("firmware");
            dir::clear(&fw_dir).unwrap();
            Tar::xf(&aarch64_tar, Some(&fw_dir)).invoke();

            let boot_dir = PathBuf::from("zCore/disk/EFI/Boot");
            dir::clear(&boot_dir).unwrap();
            fs::copy(
                fw_dir.join("aarch64_uefi.efi"),
                boot_dir.join("bootaa64.efi"),
            )
            .unwrap();
            fs::copy(fw_dir.join("Boot.json"), boot_dir.join("Boot.json")).unwrap();
        }
        // 生成镜像
        match self.0 {
            Arch::Riscv64 | Arch::Aarch64 => fuse(self.path(), &image),
            Arch::X86_64 => {
                let rootfs = self.path();
                let to = rootfs.join("lib/ld-musl-x86_64.so.1");

                // 拷贝适用于 bare-metal 的 musl_libc.so
                let musl = self.0.linux_musl_cross();
                fs::copy(
                    musl.join(format!("{}-linux-musl", self.0.name()))
                        .join("lib")
                        .join("libc.so"),
                    &to,
                )
                .unwrap();
                Ext::new(self.strip(musl)).arg("-s").arg(&to).invoke();

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

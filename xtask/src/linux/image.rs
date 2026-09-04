use crate::{Arch, PROJECT_DIR};
use os_xtask_utils::{dir, CommandExt, Ext, Qemu};
use std::{fs, path::Path};

impl super::LinuxRootfs {
    /// 生成镜像。
    pub fn image(&self) {
        // 递归 rootfs
        self.make(false);
        // 镜像路径
        let inner = PROJECT_DIR.join("zCore");
        let image = inner.join(format!("{arch}.img", arch = self.0.name()));
        // aarch64 升级为 rboot
        if let Arch::Aarch64 = self.0 {
            let boot_dir = inner.join("disk").join("EFI").join("Boot");
            dir::clear(&boot_dir).unwrap();

            Ext::new("cargo")
                .arg("build")
                .arg("--manifest-path")
                .arg("rboot/Cargo.toml")
                .arg("--target")
                .arg("aarch64-unknown-uefi")
                .arg("-Zbuild-std=core,alloc")
                .arg("-Zbuild-std-features=compiler-builtins-mem")
                .arg("--release")
                .invoke();

            let rboot_efi = PROJECT_DIR
                .join("rboot")
                .join("target")
                .join("aarch64-unknown-uefi")
                .join("release")
                .join("rboot.efi");

            fs::copy(&rboot_efi, boot_dir.join("bootaa64.efi")).unwrap();
            fs::copy(inner.join("rboot.conf"), boot_dir.join("rboot.conf")).unwrap();
        }
        // 生成镜像
        fuse(self.path(), &image);
        // 扩充一些额外空间，供某些测试使用
        Qemu::img()
            .arg("resize")
            .args(["-f", "raw"])
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

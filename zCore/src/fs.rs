use alloc::sync::Arc;
use linux_object::fs::MemBuf;
use rcore_fs::vfs::FileSystem;

pub fn init_filesystem(ramfs_data: &'static mut [u8]) -> Arc<dyn FileSystem> {
    #[cfg(target_arch = "x86_64")]
    let device = Arc::new(MemBuf::new(ramfs_data));

    #[cfg(feature = "link_user_img")]
    let ramfs_data = unsafe {
        extern "C" {
            fn _user_img_start();
            fn _user_img_end();
        }

        core::slice::from_raw_parts_mut(
            _user_img_start as *mut u8,
            _user_img_end as usize - _user_img_start as usize,
        )
    };

    #[cfg(feature = "link_user_img")]
    let device = Arc::new(MemBuf::new(ramfs_data));

    #[cfg(all(target_arch = "riscv64", not(feature = "link_user_img")))]
    let device = {
        use kernel_hal::drivers::virtio::{BlockDriverWrapper, BLK_DRIVERS};
        let driver = BlockDriverWrapper(
            BLK_DRIVERS
                .read()
                .iter()
                .next()
                .expect("Block device not found")
                .clone(),
        );
        Arc::new(rcore_fs::dev::block_cache::BlockCache::new(driver, 0x100))
    };

    info!("Opening the rootfs ...");
    // 输入类型: Arc<Device>
    let rootfs =
        rcore_fs_sfs::SimpleFileSystem::open(device).expect("failed to open device SimpleFS");

    rootfs
}

// Hard link rootfs img
#[cfg(feature = "link_user_img")]
global_asm!(concat!(
    r#"
	.section .data.img
	.global _user_img_start
	.global _user_img_end
_user_img_start:
    .incbin ""#,
    env!("USER_IMG"),
    r#""
_user_img_end:
"#
));

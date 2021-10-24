#[allow(dead_code)]
pub fn init_ram_disk() -> &'static mut [u8] {
    if cfg!(feature = "link_user_img") {
        extern "C" {
            fn _user_img_start();
            fn _user_img_end();
        }
        unsafe {
            core::slice::from_raw_parts_mut(
                _user_img_start as *mut u8,
                _user_img_end as usize - _user_img_start as usize,
            )
        }
    } else {
        kernel_hal::boot::init_ram_disk().expect("failed to get init RAM disk data")
    }
}

cfg_if! {
    if #[cfg(feature = "linux")] {
        use alloc::sync::Arc;
        use rcore_fs::vfs::FileSystem;

        #[cfg(feature = "libos")]
        pub fn rootfs() -> Arc<dyn FileSystem> {
            let base = if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
                std::path::Path::new(&dir).join("..")
            } else {
                std::env::current_dir().unwrap()
            };
            rcore_fs_hostfs::HostFS::new(base.join("rootfs"))
        }

        #[cfg(not(feature = "libos"))]
        pub fn rootfs() -> Arc<dyn FileSystem> {
            use rcore_fs::dev::Device;
            use linux_object::fs::rcore_fs_wrapper::{MemBuf, Block, BlockCache};

            let device: Arc<dyn Device> = if cfg!(feature = "init_ram_disk") {
                Arc::new(MemBuf::new(init_ram_disk()))
            } else {
                let block = kernel_hal::drivers::all_block().first_unwrap();
                Arc::new(BlockCache::new(Block::new(block), 0x100))
            };
            info!("Opening the rootfs...");
            rcore_fs_sfs::SimpleFileSystem::open(device).expect("failed to open device SimpleFS")
        }
    }
}

// Hard link rootfs img
#[cfg(not(feature = "libos"))]
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

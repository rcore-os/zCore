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

        use kernel_hal::drivers::scheme::BlockScheme;
        use rcore_fs::dev::{BlockDevice, DevError, Device, Result};
        use rcore_fs::vfs::FileSystem;

        struct BlockDriverWrapper(Arc<dyn BlockScheme>);

        impl BlockDevice for BlockDriverWrapper {
            const BLOCK_SIZE_LOG2: u8 = 9; // 512

            fn read_at(&self, block_id: usize, buf: &mut [u8]) -> Result<()> {
                self.0.read_block(block_id, buf).map_err(|_| DevError)
            }

            fn write_at(&self, block_id: usize, buf: &[u8]) -> Result<()> {
                self.0.write_block(block_id, buf).map_err(|_| DevError)
            }

            fn sync(&self) -> Result<()> {
                self.0.flush().map_err(|_| DevError)
            }
        }

        pub fn rootfs() -> Arc<dyn FileSystem> {
            let device: Arc<dyn Device> = if cfg!(feature = "init_ram_disk") {
                Arc::new(linux_object::fs::MemBuf::new(init_ram_disk()))
            } else {
                use rcore_fs::dev::block_cache::BlockCache;
                let block = kernel_hal::drivers::all_block().first_unwrap();
                Arc::new(BlockCache::new(BlockDriverWrapper(block), 0x100))
            };

            info!("Opening the rootfs...");
            rcore_fs_sfs::SimpleFileSystem::open(device).expect("failed to open device SimpleFS")
        }
    }
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

#![allow(unused_variables)]

use alloc::sync::Arc;

use rcore_fs::dev::{BlockDevice, DevError, Result};
use rcore_fs::vfs::FileSystem;
use kernel_hal::drivers::scheme::BlockScheme;

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

pub fn init_filesystem(ramfs_data: &'static mut [u8]) -> Arc<dyn FileSystem> {
    #[cfg(feature = "ramfs")]
    let device = {
        use linux_object::fs::MemBuf;
        extern "C" {
            fn _user_img_start();
            fn _user_img_end();
        }

        #[cfg(feature = "link_user_img")]
        let ramfs_data = unsafe {
            core::slice::from_raw_parts_mut(
                _user_img_start as *mut u8,
                _user_img_end as usize - _user_img_start as usize,
            )
        };
        MemBuf::new(ramfs_data)
    };

    #[cfg(not(feature = "ramfs"))]
    let device = {
        use rcore_fs::dev::block_cache::BlockCache;
        let block = kernel_hal::drivers::block::first_unwrap();
        BlockCache::new(BlockDriverWrapper(block), 0x100)
    };

    info!("Opening the rootfs ...");
    rcore_fs_sfs::SimpleFileSystem::open(Arc::new(device)).expect("failed to open device SimpleFS")
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

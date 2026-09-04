cfg_if! {
    if #[cfg(feature = "linux")] {
        use alloc::sync::Arc;
        use rcore_fs::vfs::FileSystem;

        #[cfg(feature = "libos")]
        #[cfg_attr(feature = "zircon", allow(dead_code))]
        pub fn rootfs() -> Arc<dyn FileSystem> {
            let rootfs = std::env::var_os("ZCORE_LIBOS_ROOTFS")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    let workspace = if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
                        std::path::Path::new(&dir).parent().unwrap().to_path_buf()
                    } else {
                        std::env::current_dir().unwrap()
                    };
                    workspace.join("rootfs").join("libos")
                });
            rcore_fs_hostfs::HostFS::new(rootfs)
        }

        #[cfg(not(feature = "libos"))]
        pub fn rootfs() -> Arc<dyn FileSystem> {
            use rcore_fs::dev::Device;

            let device: Arc<dyn Device> = {
                #[cfg(feature = "mock-disk")]{
                    let block = linux_object::fs::mock_block();
                    Arc::new(block)
                }
                #[cfg(not(feature = "mock-disk"))] {
                    use linux_object::fs::rcore_fs_wrapper::*;
                    if let Some(initrd) = init_ram_disk() {
                        Arc::new(MemBuf::new(initrd))
                    } else {
                        let block = kernel_hal::drivers::all_block().first_unwrap();
                        Arc::new(BlockCache::new(Block::new(block), 0x100))
                    }
                }
            };
            info!("Opening the rootfs...");
            rcore_fs_sfs::SimpleFileSystem::open(device).expect("failed to open device SimpleFS")
        }
    } else if #[cfg(feature = "zircon")] {

        #[cfg(feature = "libos")]
        pub fn zbi() -> impl AsRef<[u8]> {
            let path = std::env::args().nth(1).unwrap();
            std::fs::read(path).expect("failed to read zbi file")
        }

        #[cfg(not(feature = "libos"))]
        pub fn zbi() -> impl AsRef<[u8]> {
            init_ram_disk().expect("failed to get the init RAM disk")
        }
    }
}

#[cfg(not(feature = "libos"))]
pub(crate) fn init_ram_disk() -> Option<&'static mut [u8]> {
    if cfg!(feature = "link-user-img") {
        unsafe extern "C" {
            fn _user_img_start();
            fn _user_img_end();
        }
        Some(unsafe {
            core::slice::from_raw_parts_mut(
                _user_img_start as *mut u8,
                _user_img_end as *const () as usize - _user_img_start as *const () as usize,
            )
        })
    } else {
        kernel_hal::boot::init_ram_disk()
    }
}

// Hard link rootfs img
#[cfg(not(feature = "libos"))]
#[cfg(feature = "link-user-img")]
core::arch::global_asm!(concat!(
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

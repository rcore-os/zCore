cfg_if! {
    if #[cfg(feature = "linux")] {
        use alloc::sync::Arc;
        use rcore_fs::vfs::FileSystem;

        #[cfg(feature = "libos")]
        #[cfg_attr(feature = "zircon", allow(dead_code))]
        pub fn rootfs() -> Arc<dyn FileSystem> {
            let  rootfs = if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
                std::path::Path::new(&dir).parent().unwrap().to_path_buf()
            } else {
                std::env::current_dir().unwrap()
            };
            rcore_fs_hostfs::HostFS::new(rootfs.join("rootfs").join("libos"))
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
#[cfg(feature = "link-user-img")]
pub(crate) fn init_ram_disk() -> Option<&'static mut [u8]> {
    Some(unsafe {
        core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(USER_IMG.0).cast(), USER_IMG_LEN)
    })
}

#[cfg(all(not(feature = "libos"), not(feature = "link-user-img")))]
pub(crate) fn init_ram_disk() -> Option<&'static mut [u8]> {
    kernel_hal::boot::init_ram_disk()
}

// Embed the rootfs image in the kernel for platforms without an initrd handoff.
#[cfg(not(feature = "libos"))]
#[cfg(feature = "link-user-img")]
const USER_IMG_LEN: usize = include_bytes!(env!("USER_IMG")).len();

#[cfg(not(feature = "libos"))]
#[cfg(feature = "link-user-img")]
#[repr(align(4096))]
struct AlignedUserImage([u8; USER_IMG_LEN]);

#[cfg(not(feature = "libos"))]
#[cfg(feature = "link-user-img")]
#[link_section = ".data.img"]
static mut USER_IMG: AlignedUserImage = AlignedUserImage(*include_bytes!(env!("USER_IMG")));

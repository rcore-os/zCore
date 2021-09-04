pub use crate::common::fb::*;

#[repr(C)]
#[derive(Debug, Default)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: u64,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: u64,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

impl FbFixScreeninfo {
    pub fn size(&self) -> u32 {
        self.smem_len
    }
}

#[repr(C)]
#[derive(Debug, Default)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

impl FbVarScreeninfo {
    pub fn resolution(&self) -> (u32, u32) {
        (self.xres, self.yres)
    }
}

#[repr(C)]
#[derive(Debug, Default)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

hal_fn_impl! {
    impl mod crate::defs::dev::fb {
        fn init() {
            const FBIOGET_VSCREENINFO: u64 = 0x4600;
            const FBIOGET_FSCREENINFO: u64 = 0x4602;

            #[cfg(target_arch = "aarch64")]
            let fbfd = unsafe { libc::open("/dev/fb0".as_ptr(), libc::O_RDWR) };
            #[cfg(not(target_arch = "aarch64"))]
            let fbfd = unsafe { libc::open("/dev/fb0".as_ptr() as *const i8, libc::O_RDWR) };
            if fbfd < 0 {
                return;
            }

            let mut vinfo = FbVarScreeninfo::default();
            if unsafe { libc::ioctl(fbfd, FBIOGET_VSCREENINFO, &mut vinfo) } < 0 {
                return;
            }

            let mut finfo = FbFixScreeninfo::default();
            if unsafe { libc::ioctl(fbfd, FBIOGET_FSCREENINFO, &mut finfo) } < 0 {
                return;
            }

            let size = finfo.size() as usize;
            let addr = unsafe {
                libc::mmap(
                    std::ptr::null_mut::<libc::c_void>(),
                    size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fbfd,
                    0,
                )
            };
            if (addr as isize) < 0 {
                return;
            }

            let (width, height) = vinfo.resolution();
            let addr = addr as usize;

            let fb_info = FramebufferInfo {
                xres: width,
                yres: height,
                xres_virtual: width,
                yres_virtual: height,
                xoffset: 0,
                yoffset: 0,
                depth: ColorDepth::ColorDepth32,
                format: ColorFormat::RGBA8888,
                // paddr: virt_to_phys(addr),
                paddr: addr,
                vaddr: addr,
                screen_size: size,
            };
            *FRAME_BUFFER.write() = Some(fb_info);
        }
    }
}

use spin::RwLock;

pub static FRAME_BUFFER: RwLock<Option<FramebufferInfo>> = RwLock::new(None);

/// FramebufferInfo information
#[repr(C)]
#[derive(Debug)]
pub struct FramebufferInfo {
    /// visible width
    pub xres: u32,
    /// visible height
    pub yres: u32,
    /// virtual width
    pub xres_virtual: u32,
    /// virtual height
    pub yres_virtual: u32,
    /// virtual offset x
    pub xoffset: u32,
    /// virtual offset y
    pub yoffset: u32,

    /// bits per pixel
    pub depth: ColorDepth,
    /// color encoding format of RGBA
    pub format: ColorFormat,

    /// phsyical address
    pub paddr: usize,
    /// virtual address
    pub vaddr: usize,
    /// screen buffer size
    pub screen_size: usize,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum ColorDepth {
    ColorDepth8 = 8,
    ColorDepth16 = 16,
    ColorDepth24 = 24,
    ColorDepth32 = 32,
}

impl ColorDepth {
    pub fn try_from(depth: u32) -> Result<Self, &'static str> {
        match depth {
            8 => Ok(Self::ColorDepth8),
            16 => Ok(Self::ColorDepth16),
            32 => Ok(Self::ColorDepth32),
            24 => Ok(Self::ColorDepth24),
            _ => Err("unsupported color depth"),
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum ColorFormat {
    RGB332,
    RGB565,
    RGBA8888, // QEMU and low version RPi use RGBA
    BGRA8888, // RPi3 B+ uses BGRA
    VgaPalette,
}

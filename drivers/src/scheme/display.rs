use super::Scheme;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    ColorDepth8 = 8,
    ColorDepth16 = 16,
    ColorDepth24 = 24,
    ColorDepth32 = 32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorFormat {
    RGB332,
    RGB565,
    RGBA8888, // QEMU and low version RPi use RGBA
    BGRA8888, // RPi3 B+ uses BGRA
    VgaPalette,
}

#[derive(Debug, Clone, Copy)]
pub struct DisplayInfo {
    /// visible width
    pub width: u32,
    /// visible height
    pub height: u32,
    /// frame buffer size
    pub fb_size: usize,

    /// bits per pixel
    pub depth: ColorDepth,
    /// color encoding format of RGBA
    pub format: ColorFormat,
}

impl ColorDepth {
    pub fn try_from(depth: u8) -> Result<Self, &'static str> {
        match depth {
            8 => Ok(Self::ColorDepth8),
            16 => Ok(Self::ColorDepth16),
            24 => Ok(Self::ColorDepth24),
            32 => Ok(Self::ColorDepth32),
            _ => Err("unsupported color depth"),
        }
    }

    pub fn bytes(self) -> u8 {
        self as u8 / 8
    }
}

pub trait DisplayScheme: Scheme {
    fn info(&self) -> DisplayInfo;

    #[allow(clippy::mut_from_ref)]
    unsafe fn raw_fb(&self) -> &mut [u8];
}

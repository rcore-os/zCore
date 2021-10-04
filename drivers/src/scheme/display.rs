use super::Scheme;
use crate::{DeviceError, DeviceResult};

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor(u32);

impl RgbColor {
    #[inline]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    #[inline]
    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }

    #[inline]
    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }

    #[inline]
    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    #[inline]
    pub const fn raw_value(self) -> u32 {
        self.0
    }
}

/// Color format for one pixel. `RGB888` means R in bits 16-23, G in bits 8-15 and B in bits 0-7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorFormat {
    RGB332,
    RGB565,
    RGB888,
    RGBA8888, // QEMU and low version RPi use RGBA
    BGRA8888, // RPi3 B+ uses BGRA
}

impl ColorFormat {
    /// Number of bits per pixel.
    #[inline]
    pub const fn depth(self) -> u8 {
        match self {
            Self::RGB332 => 8,
            Self::RGB565 => 16,
            Self::RGB888 => 24,
            Self::RGBA8888 => 32,
            Self::BGRA8888 => 32,
        }
    }

    /// Number of bytes per pixel.
    #[inline]
    pub const fn bytes(self) -> u8 {
        self.depth() / 8
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DisplayInfo {
    /// visible width
    pub width: u32,
    /// visible height
    pub height: u32,
    /// color encoding format of RGBA
    pub format: ColorFormat,
    /// frame buffer size
    pub fb_size: usize,
}

impl DisplayInfo {
    /// Number of bytes between each row of the frame buffer.
    #[inline]
    pub const fn pitch(self) -> u32 {
        self.width * self.format.bytes() as u32
    }
}

pub trait DisplayScheme: Scheme {
    fn info(&self) -> DisplayInfo;

    #[allow(clippy::mut_from_ref)]
    /// Returns the raw framebuffer.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it returns the raw pointer of the framebuffer.
    unsafe fn raw_fb(&self) -> &mut [u8];

    #[inline]
    fn write_pixel(&self, x: u32, y: u32, color: RgbColor) -> DeviceResult {
        let info = self.info();
        let fb = unsafe { self.raw_fb() };
        let offset = (x + y * info.width) as usize * info.format.bytes() as usize;
        if offset >= info.fb_size {
            return Err(DeviceError::InvalidParam);
        }
        unsafe { write_color(&mut fb[offset as usize] as _, color, info.format) };
        Ok(())
    }
}

const fn pack_channel(r_val: u8, _r_bits: u8, g_val: u8, g_bits: u8, b_val: u8, b_bits: u8) -> u32 {
    ((r_val as u32) << (g_bits + b_bits)) | ((g_val as u32) << b_bits) | b_val as u32
}

unsafe fn write_color(ptr: *mut u8, color: RgbColor, format: ColorFormat) {
    let (r, g, b) = (color.r(), color.g(), color.b());
    let dst = core::slice::from_raw_parts_mut(ptr, 4);
    match format {
        ColorFormat::RGB332 => {
            *ptr = pack_channel(r >> (8 - 3), 3, g >> (8 - 3), 3, b >> (8 - 2), 2) as u8
        }
        ColorFormat::RGB565 => {
            *(ptr as *mut u16) =
                pack_channel(r >> (8 - 5), 5, g >> (8 - 6), 6, b >> (8 - 5), 5) as u16
        }
        ColorFormat::RGB888 => {
            dst[2] = r;
            dst[1] = g;
            dst[0] = b;
        }
        ColorFormat::RGBA8888 => *(ptr as *mut u32) = color.raw_value() << 8,
        ColorFormat::BGRA8888 => {
            dst[3] = b;
            dst[2] = g;
            dst[1] = r;
        }
    }
}

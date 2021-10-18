use alloc::vec::Vec;

use crate::prelude::{ColorFormat, DisplayInfo, FrameBuffer};
use crate::scheme::{DisplayScheme, Scheme};

pub struct MockDisplay {
    info: DisplayInfo,
    fb: Vec<u8>,
}

impl MockDisplay {
    pub fn new(width: u32, height: u32, format: ColorFormat) -> Self {
        let fb_size = (width * height * format.bytes() as u32) as usize;
        let fb = vec![0; fb_size];
        let info = DisplayInfo {
            width,
            height,
            format,
            fb_base_vaddr: fb.as_ptr() as usize,
            fb_size,
        };
        Self { info, fb }
    }

    /// # Safety
    ///
    /// This function is unsafe, the caller must ensure the `ptr` points to the
    /// start of a valid frame buffer.
    pub unsafe fn from_raw_parts(
        width: u32,
        height: u32,
        format: ColorFormat,
        ptr: *mut u8,
    ) -> Self {
        let fb_size = (width * height * format.bytes() as u32) as usize;
        let fb = Vec::from_raw_parts(ptr, fb_size, fb_size);
        let info = DisplayInfo {
            width,
            height,
            format,
            fb_base_vaddr: fb.as_ptr() as usize,
            fb_size,
        };
        Self { info, fb }
    }
}

impl Scheme for MockDisplay {
    fn name(&self) -> &str {
        "mock-display"
    }
}

impl DisplayScheme for MockDisplay {
    #[inline]
    fn info(&self) -> DisplayInfo {
        self.info
    }

    #[inline]
    fn fb(&self) -> FrameBuffer {
        unsafe { FrameBuffer::from_raw_parts_mut(self.fb.as_ptr() as _, self.info.fb_size) }
    }
}

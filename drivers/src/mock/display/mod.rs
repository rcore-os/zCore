pub mod sdl;

use alloc::vec::Vec;

use crate::display::{ColorDepth, ColorFormat, DisplayInfo};
use crate::scheme::{DisplayScheme, Scheme};

pub struct MockDisplay {
    info: DisplayInfo,
    fb: Vec<u8>,
}

impl MockDisplay {
    pub fn new(width: u32, height: u32) -> Self {
        let depth = ColorDepth::ColorDepth32;
        let format = ColorFormat::RGBA8888;
        let fb_size = (width * height * depth.bytes() as u32) as usize;
        let info = DisplayInfo {
            width,
            height,
            fb_size,
            depth,
            format,
        };
        let fb = vec![0; fb_size];
        Self { info, fb }
    }
}

impl Scheme for MockDisplay {
    fn name(&self) -> &str {
        "mock-display"
    }
}

impl DisplayScheme for MockDisplay {
    fn info(&self) -> DisplayInfo {
        self.info
    }

    unsafe fn raw_fb(&self) -> &mut [u8] {
        core::slice::from_raw_parts_mut(self.fb.as_ptr() as _, self.info.fb_size)
    }
}

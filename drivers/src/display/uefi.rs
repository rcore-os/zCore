//! UEFI Graphics Output Protocol

use crate::prelude::{DisplayInfo, FrameBuffer};
use crate::scheme::{DisplayScheme, Scheme};

pub struct UefiDisplay {
    info: DisplayInfo,
}

impl UefiDisplay {
    pub fn new(info: DisplayInfo) -> Self {
        Self { info }
    }
}

impl Scheme for UefiDisplay {
    fn name(&self) -> &str {
        "mock-display"
    }
}

impl DisplayScheme for UefiDisplay {
    #[inline]
    fn info(&self) -> DisplayInfo {
        self.info
    }

    #[inline]
    fn fb(&self) -> FrameBuffer {
        unsafe {
            FrameBuffer::from_raw_parts_mut(self.info.fb_base_vaddr as *mut u8, self.info.fb_size)
        }
    }
}

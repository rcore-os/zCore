use spin::Mutex;
use virtio_drivers::{VirtIOGpu as InnerDriver, VirtIOHeader};

use crate::prelude::{ColorFormat, DisplayInfo, FrameBuffer};
use crate::scheme::{DisplayScheme, Scheme};
use crate::DeviceResult;

pub struct VirtIoGpu<'a> {
    info: DisplayInfo,
    inner: Mutex<InnerDriver<'a>>,
}

const CURSOR_HOT_X: u32 = 13;
const CURSOR_HOT_Y: u32 = 11;
static CURSOR_IMG: &[u8] = include_bytes!("../display/resource/cursor.bin"); // 64 x 64 x 4

impl<'a> VirtIoGpu<'a> {
    pub fn new(header: &'static mut VirtIOHeader) -> DeviceResult<Self> {
        let mut gpu = InnerDriver::new(header)?;
        let fb = gpu.setup_framebuffer()?;
        let fb_base_vaddr = fb.as_ptr() as usize;
        let fb_size = fb.len();
        let (width, height) = gpu.resolution();
        let info = DisplayInfo {
            width,
            height,
            format: ColorFormat::ARGB8888,
            fb_base_vaddr,
            fb_size,
        };
        gpu.setup_cursor(
            CURSOR_IMG,
            width / 2,
            height / 2,
            CURSOR_HOT_X,
            CURSOR_HOT_Y,
        )?;
        Ok(Self {
            info,
            inner: Mutex::new(gpu),
        })
    }
}

impl<'a> Scheme for VirtIoGpu<'a> {
    fn name(&self) -> &str {
        "virtio-gpu"
    }

    fn handle_irq(&self, _irq_num: usize) {
        self.inner.lock().ack_interrupt();
    }
}

impl<'a> DisplayScheme for VirtIoGpu<'a> {
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

    #[inline]
    fn need_flush(&self) -> bool {
        true
    }

    fn flush(&self) -> DeviceResult {
        self.inner.lock().flush()?;
        Ok(())
    }
}

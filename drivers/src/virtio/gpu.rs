use alloc::vec::Vec;
use lock::Mutex;
use virtio_drivers::{VirtIOGpu as InnerDriver, VirtIOHeader};

use crate::prelude::{AccelCaps, ColorFormat, DisplayInfo, FrameBuffer};
use crate::scheme::{DisplayScheme, DrmScheme, Scheme};
use crate::DeviceResult;

pub struct VirtIoGpu<'a> {
    info: DisplayInfo,
    inner: Option<Mutex<InnerDriver<'a>>>,
}

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
            pitch: width * 4,
            format: ColorFormat::ARGB8888,
            fb_base_vaddr,
            fb_size,
        };
        Ok(Self {
            info,
            inner: Some(Mutex::new(gpu)),
        })
    }

    /// Initialize a VirtIO GPU in Modern mode (PCI)
    pub fn new_modern(
        common_vaddr: usize,
        _device_vaddr: usize,
        _notify_vaddr: usize,
        fb_vaddr: usize,
        fb_size: usize,
    ) -> DeviceResult<Self> {
        let common = unsafe { &mut *(common_vaddr as *mut VirtioPciCommonCfg) };

        // Basic initialization for Modern PCI
        common.device_status = 0; // Reset
        common.device_status |= 1; // ACK
        common.device_status |= 2; // DRIVER
        common.device_status |= 8; // FEATURES_OK
        common.device_status |= 4; // DRIVER_OK

        let info = DisplayInfo {
            width: 1024,
            height: 768,
            pitch: 1024 * 4,
            format: ColorFormat::ARGB8888,
            fb_base_vaddr: fb_vaddr,
            fb_size: if fb_size > 0 { fb_size } else { 1024 * 768 * 4 },
        };

        // In Modern mode, we don't use the legacy InnerDriver because it requires a legacy header.
        Ok(Self { info, inner: None })
    }
}

#[repr(C)]
struct VirtioPciCommonCfg {
    device_feature_select: u32,
    device_feature: u32,
    driver_feature_select: u32,
    driver_feature: u32,
    msix_config: u16,
    num_queues: u16,
    device_status: u8,
    config_generation: u8,
    queue_select: u16,
    queue_size: u16,
    queue_msix_vector: u16,
    queue_enable: u16,
    queue_notify_off: u16,
    queue_desc: u64,
    queue_driver: u64,
    queue_device: u64,
}

impl<'a> Scheme for VirtIoGpu<'a> {
    fn name(&self) -> &str {
        "virtio-gpu"
    }

    fn handle_irq(&self, _irq_num: usize) {
        if let Some(inner) = &self.inner {
            inner.lock().ack_interrupt();
        }
    }
}

impl<'a> DisplayScheme for VirtIoGpu<'a> {
    fn info(&self) -> DisplayInfo {
        self.info
    }

    fn fb(&self) -> FrameBuffer<'_> {
        unsafe {
            FrameBuffer::from_raw_parts_mut(self.info.fb_base_vaddr as *mut u8, self.info.fb_size)
        }
    }

    /// The framebuffer is host-shared memory: the generic 2D primitives fill /
    /// copy / blit it in bulk in RAM and a single [`flush`](Self::flush) hands
    /// the dirty frame to the host (QEMU / VirtualBox) for display.
    fn accel_caps(&self) -> AccelCaps {
        AccelCaps {
            fill: true,
            copy: true,
            blit: true,
        }
    }

    fn need_flush(&self) -> bool {
        self.inner.is_some()
    }

    fn flush(&self) -> DeviceResult {
        if let Some(inner) = &self.inner {
            inner.lock().flush()?;
        }
        Ok(())
    }
}

use crate::scheme::drm::{DrmCaps, DrmConnector, DrmCrtc, DrmPlane, GemHandle};

impl<'a> DrmScheme for VirtIoGpu<'a> {
    fn get_caps(&self) -> DrmCaps {
        DrmCaps {
            has_3d: true,
            has_cursor: true,
            max_width: self.info.width,
            max_height: self.info.height,
        }
    }

    fn import_buffer(&self, _handle: GemHandle) -> bool {
        true
    }

    fn free_buffer(&self, _handle: GemHandle) {}

    fn create_fb(&self, handle_id: u32, _width: u32, _height: u32, _pitch: u32) -> Option<u32> {
        Some(handle_id)
    }

    fn page_flip(&self, _fb_id: u32) -> bool {
        self.flush().is_ok()
    }

    fn set_cursor(&self, _crtc_id: u32, _x: i32, _y: i32, _handle: u32, _flags: u32) -> bool {
        true
    }

    fn wait_vblank(&self, _crtc_id: u32) -> bool {
        true
    }

    fn get_resources(&self) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
        (Vec::new(), alloc::vec![2000], alloc::vec![1000])
    }

    fn get_connector(&self, id: u32) -> Option<DrmConnector> {
        if id == 1000 {
            // Compute physical dimensions from the display resolution at ~96 DPI
            // (25.4 mm/inch, 96 px/inch). Use .max(1) to ensure non-zero values
            // so compositors do not see "Physical size: 0x0" warnings.
            let info = self.info();
            let mm_width = (info.width * 254 / 960).max(1);
            let mm_height = (info.height * 254 / 960).max(1);
            Some(DrmConnector {
                id,
                connected: true,
                mm_width,
                mm_height,
                connector_type: 11,
            })
        } else {
            None
        }
    }

    fn get_crtc(&self, id: u32) -> Option<DrmCrtc> {
        if id == 2000 {
            Some(DrmCrtc {
                id,
                fb_id: 0,
                x: 0,
                y: 0,
            })
        } else {
            None
        }
    }

    fn get_plane(&self, id: u32) -> Option<DrmPlane> {
        if id == 3000 {
            Some(DrmPlane {
                id,
                crtc_id: 2000,
                fb_id: 0,
                possible_crtcs: 1,
                plane_type: 1,
            })
        } else {
            None
        }
    }

    fn get_planes(&self) -> Vec<u32> {
        alloc::vec![3000]
    }

    fn set_plane(
        &self,
        _plane_id: u32,
        _crtc_id: u32,
        _fb_id: u32,
        _x: i32,
        _y: i32,
        _w: u32,
        _h: u32,
        _src_x: u32,
        _src_y: u32,
        _src_w: u32,
        _src_h: u32,
    ) -> bool {
        true
    }

    fn ioctl(&self, request: u32, _arg: usize) -> Result<usize, i32> {
        const DRM_IOCTL_VIRTGPU_GETPARAM: u32 = 0xC0106443;
        match request {
            DRM_IOCTL_VIRTGPU_GETPARAM => Ok(0),
            _ => Err(38),
        }
    }
}

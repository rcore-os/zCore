use kernel_hal::drivers::{BlockDriver, DeviceType, Driver, BLK_DRIVERS, DRIVERS};

/// Block device
pub mod virtio;

/////////
pub trait GpuDriver: Driver {
    fn resolution(&self) -> (u32, u32) {
        unimplemented!("not a gpu driver")
    }

    fn setup_framebuffer(&self) -> (usize, usize) {
        unimplemented!("not a gpu driver")
    }

    fn flush(&self) -> virtio_drivers::Result {
        unimplemented!("not a gpu driver")
    }
}

pub trait InputDriver: Driver {
    fn mouse_xy(&self) -> (i32, i32) {
        unimplemented!("not a input driver")
    }
}
/////////

use crate::drivers::Driver;
//pub use block::BlockDriver;

/// Block device
pub mod virtio;

/////////
pub trait BlockDriver: Driver {
    fn read_block(&self, _block_id: usize, _buf: &mut [u8]) -> bool {
        unimplemented!("not a block driver")
    }

    fn write_block(&self, _block_id: usize, _buf: &[u8]) -> bool {
        unimplemented!("not a block driver")
    }
}

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

#![no_std]
#![feature(asm)]

extern crate alloc;

#[cfg(feature = "virtio")]
mod virtio;

pub mod block;
pub mod io;
pub mod scheme;
pub mod uart;

#[derive(Debug)]
pub enum DeviceError {
    /// The buffer is too small.
    BufferTooSmall,
    /// The device is not ready.
    NotReady,
    /// Invalid parameter.
    InvalidParam,
    /// Failed to alloc DMA memory.
    DmaError,
    /// I/O Error
    IoError,
}

pub type DeviceResult<T = ()> = core::result::Result<T, DeviceError>;

pub type IrqHandler = alloc::boxed::Box<dyn Fn() + Send + Sync>;

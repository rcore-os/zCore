#![cfg_attr(not(feature = "mock"), no_std)]
#![feature(asm)]

extern crate alloc;

#[macro_use]
extern crate log;

#[cfg(feature = "mock")]
pub mod mock;

#[cfg(feature = "virtio")]
pub mod virtio;

pub mod io;
pub mod irq;
pub mod scheme;
pub mod uart;
pub mod utils;

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
    /// A resource with the specified identifier already exists.
    AlreadyExists,
    /// No resource to allocate.
    NoResources,
}

pub type DeviceResult<T = ()> = core::result::Result<T, DeviceError>;

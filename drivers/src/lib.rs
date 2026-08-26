//! Device drivers of zCore.

#![cfg_attr(not(feature = "mock"), no_std)]
#![deny(warnings)]
// The pinned nightly's clippy flags a batch of style lints across the legacy
// driver code (register-offset tables written `0x0000/4` for column alignment,
// MMIO `transmute`s, wide hardware-init signatures, driver doc formatting).
// They are intentional low-level-driver patterns, not defects; blanket-allowing
// them here keeps `deny(warnings)` meaningful for real issues without churning
// vendored-style driver internals.
#![allow(clippy::erasing_op)] // `0x0000 / 4` register offset kept for alignment
#![allow(clippy::manual_clamp)] // explicit min/max on input deltas reads clearer
#![allow(clippy::needless_range_loop)] // index-parallel hardware descriptor walks
#![allow(clippy::missing_safety_doc)] // legacy unsafe MMIO helpers
#![allow(clippy::missing_transmute_annotations)] // MMIO register transmutes
#![allow(clippy::type_complexity)] // driver callback/handler tuples
#![allow(clippy::too_many_arguments)] // hardware bring-up entry points
#![allow(clippy::doc_lazy_continuation)] // driver doc-comment wrapping
#![feature(doc_cfg)]

extern crate alloc;

#[macro_use]
extern crate log;

use alloc::sync::Arc;
use core::fmt;

#[cfg(any(feature = "mock", doc))]
#[doc(cfg(feature = "mock"))]
pub mod mock;

#[cfg(any(feature = "virtio", doc))]
#[doc(cfg(feature = "virtio"))]
pub mod virtio;

pub mod ata;
pub mod audio;
pub mod builder;
#[macro_use]
pub mod bus;
pub mod display;
pub mod input;
pub mod io;
pub mod irq;
pub mod net;
pub mod nvme;
pub mod prelude;
pub mod scheme;
pub mod uart;
pub mod utils;

#[cfg(all(
    any(feature = "xhci-usb-hid", feature = "legacy-usb-hid"),
    target_arch = "x86_64",
    not(feature = "mock"),
    not(feature = "no-pci")
))]
pub mod usb;

/// Initialize all drivers.
pub fn init() {
    #[cfg(any(target_arch = "x86_64", target_arch = "riscv64"))]
    bus::pci_drivers::init_all();
}

/// The error type for external device.
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
    /// The device driver is not implemented, supported, or enabled.
    NotSupported,
}

/// A type alias for the result of a device operation.
pub type DeviceResult<T = ()> = core::result::Result<T, DeviceError>;

/// Static shell of shared dynamic device [`Scheme`](crate::scheme::Scheme) types.
#[derive(Clone)]
pub enum Device {
    /// Block device
    Block(Arc<dyn scheme::BlockScheme>),
    /// Display device
    Display(Arc<dyn scheme::DisplayScheme>),
    /// Input device
    Input(Arc<dyn scheme::InputScheme>),
    /// Interrupt request and handle
    Irq(Arc<dyn scheme::IrqScheme>),
    /// Network device
    Net(Arc<dyn scheme::NetScheme>),
    /// Uart port
    Uart(Arc<dyn scheme::UartScheme>),
    /// DRM device
    Drm(Arc<dyn scheme::DrmScheme>),
    /// PCM audio output device
    Audio(Arc<dyn scheme::AudioScheme>),
}

impl Device {
    /// Get a general [`Scheme`](scheme::Scheme) from the device.
    pub fn inner(&self) -> Arc<dyn scheme::Scheme> {
        match self {
            Self::Block(d) => d.clone().upcast(),
            Self::Display(d) => d.clone().upcast(),
            Self::Input(d) => d.clone().upcast(),
            Self::Irq(d) => d.clone().upcast(),
            Self::Net(d) => d.clone().upcast(),
            Self::Uart(d) => d.clone().upcast(),
            Self::Drm(d) => d.clone().upcast(),
            Self::Audio(d) => d.clone().upcast(),
        }
    }
}

impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Block(d) => write!(f, "BlockDevice({:?})", d.name()),
            Self::Display(d) => write!(f, "DisplayDevice({:?})", d.name()),
            Self::Input(d) => write!(f, "InputDevice({:?})", d.name()),
            Self::Irq(d) => write!(f, "IrqDevice({:?})", d.name()),
            Self::Net(d) => write!(f, "NetDevice({:?})", d.name()),
            Self::Uart(d) => write!(f, "UartDevice({:?})", d.name()),
            Self::Drm(d) => write!(f, "DrmDevice({:?})", d.name()),
            Self::Audio(d) => write!(f, "AudioDevice({:?})", d.name()),
        }
    }
}

type PhysAddr = usize;
type VirtAddr = usize;

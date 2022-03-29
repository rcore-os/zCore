//! Event handler and device tree.

#![allow(unused_imports)]

mod event_listener;
mod id_allocator;
mod irq_manager;

#[cfg(feature = "graphic")]
mod graphic_console;

pub mod devicetree;

pub(super) use id_allocator::IdAllocator;
pub(super) use irq_manager::IrqManager;

pub use event_listener::{EventHandler, EventListener};

#[cfg(feature = "graphic")]
pub use graphic_console::GraphicConsole;

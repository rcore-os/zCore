//! Event handler and device tree.

#![allow(unused_imports)]

mod event_listener;
mod id_allocator;
mod irq_manager;

#[cfg(feature = "graphic")]
mod graphic_console;

pub mod devicetree;
pub mod lazy_init;

pub(super) use id_allocator::IdAllocator;
pub(super) use irq_manager::IrqManager;

pub use event_listener::{EventHandler, EventListener};

#[cfg(feature = "graphic")]
pub use graphic_console::GraphicConsole;

pub(crate) use lazy_init::LazyInit;

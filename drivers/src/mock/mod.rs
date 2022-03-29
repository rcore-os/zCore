//! Mock devices, including display, input, uart and graphic.

pub mod display;
pub mod input;
pub mod uart;

#[cfg(any(feature = "graphic", doc))]
#[doc(cfg(feature = "graphic"))]
pub mod graphic;

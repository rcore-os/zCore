use alloc::boxed::Box;

use zcore_drivers::scheme::{IrqScheme, UartScheme};

use crate::utils::init_once::InitOnce;

pub static UART: InitOnce<Box<dyn UartScheme>> = InitOnce::new();
pub static IRQ: InitOnce<Box<dyn IrqScheme>> = InitOnce::new();

#![allow(dead_code)]

use alloc::sync::Arc;

use zcore_drivers::scheme::{IrqScheme, UartScheme};

use crate::utils::init_once::InitOnce;

pub static UART: InitOnce<Arc<dyn UartScheme>> = InitOnce::new();
pub static IRQ: InitOnce<Arc<dyn IrqScheme>> = InitOnce::new();

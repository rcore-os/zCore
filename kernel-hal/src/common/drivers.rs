use alloc::boxed::Box;

use zcore_drivers::scheme::UartScheme;

use crate::utils::init_once::InitOnce;

pub static UART: InitOnce<Box<dyn UartScheme>> = InitOnce::new();

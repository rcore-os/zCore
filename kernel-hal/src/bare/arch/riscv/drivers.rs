use alloc::boxed::Box;

use zcore_drivers::irq::riscv::{Intc, Plic, ScauseIntCode};
use zcore_drivers::scheme::{AsScheme, EventListener, IrqScheme};
use zcore_drivers::uart::Uart16550Mmio;
use zcore_drivers::DeviceResult;

use super::{consts, trap};
use crate::drivers::{IRQ, UART};
use crate::mem::phys_to_virt;
use crate::utils::init_once::InitOnce;

static PLIC: InitOnce<Plic> = InitOnce::new();

pub(super) fn init() -> DeviceResult {
    UART.init_by(Box::new(EventListener::new(unsafe {
        Uart16550Mmio::<u8>::new(phys_to_virt(consts::UART_BASE))
    })));
    IRQ.init_by(Box::new(Intc::new()));

    PLIC.init_by(Plic::new(phys_to_virt(consts::PLIC_BASE)));
    PLIC.register_device(consts::UART0_INT_NUM, UART.as_scheme())?;

    IRQ.register_handler(
        ScauseIntCode::SupervisorSoft as _,
        Box::new(|_| trap::super_soft()),
    )?;
    IRQ.register_handler(
        ScauseIntCode::SupervisorTimer as _,
        Box::new(|_| trap::super_timer()),
    )?;
    IRQ.register_device(ScauseIntCode::SupervisorExternal as _, PLIC.as_scheme())?;

    Ok(())
}

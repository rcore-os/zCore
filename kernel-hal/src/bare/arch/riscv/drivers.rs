use alloc::boxed::Box;

use zcore_drivers::irq::{RiscvIntc, RiscvPlic, RiscvScauseIntCode};
use zcore_drivers::scheme::{AsScheme, EventListener, IrqScheme};
use zcore_drivers::uart::Uart16550Mmio;

use super::{consts, trap};
use crate::drivers::{IRQ, UART};
use crate::mem::phys_to_virt;
use crate::utils::init_once::InitOnce;

static PLIC: InitOnce<RiscvPlic> = InitOnce::new();

pub(super) fn init() {
    IRQ.init_by(Box::new(RiscvIntc::new()));
    UART.init_by(Box::new(EventListener::new(unsafe {
        Uart16550Mmio::<u8>::new(phys_to_virt(consts::UART_BASE))
    })));

    PLIC.init_by(RiscvPlic::new(phys_to_virt(consts::PLIC_BASE)));
    PLIC.register_device(consts::UART0_INT_NUM, UART.as_scheme())
        .unwrap();

    IRQ.register_handler(
        RiscvScauseIntCode::SupervisorSoft as _,
        Box::new(|_| trap::super_soft()),
    )
    .unwrap();
    IRQ.register_handler(
        RiscvScauseIntCode::SupervisorTimer as _,
        Box::new(|_| trap::super_timer()),
    )
    .unwrap();
    IRQ.register_device(
        RiscvScauseIntCode::SupervisorExternal as _,
        PLIC.as_scheme(),
    )
    .unwrap();
}

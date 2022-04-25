use riscv::register::sie;
use spin::Once;

use crate::prelude::IrqHandler;
use crate::scheme::{IrqScheme, Scheme};
use crate::{DeviceError, DeviceResult};
use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicU8, Ordering};

const S_SOFT: usize = 1;
const S_TIMER: usize = 5;
const S_EXT: usize = 9;

static INTC_NUM: AtomicU8 = AtomicU8::new(0);

#[repr(usize)]
pub enum ScauseIntCode {
    SupervisorSoft = S_SOFT,
    SupervisorTimer = S_TIMER,
    SupervisorExternal = S_EXT,
}

pub struct Intc {
    name: String,
    soft_handler: Once<IrqHandler>,
    timer_handler: Once<IrqHandler>,
    ext_handler: Once<IrqHandler>,
}

impl Intc {
    pub fn new() -> Self {
        Self {
            name: format!("riscv-intc-cpu{}", INTC_NUM.fetch_add(1, Ordering::Relaxed)),
            soft_handler: Once::new(),
            timer_handler: Once::new(),
            ext_handler: Once::new(),
        }
    }

    fn with_handler<F>(&self, cause: usize, op: F) -> DeviceResult
    where
        F: FnOnce(&Once<IrqHandler>) -> DeviceResult,
    {
        match cause {
            S_SOFT => op(&self.soft_handler),
            S_TIMER => op(&self.timer_handler),
            S_EXT => op(&self.ext_handler),
            _ => {
                error!("invalid SCAUSE value {:#x}!", cause);
                Err(DeviceError::InvalidParam)
            }
        }
    }
}

impl Default for Intc {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheme for Intc {
    fn name(&self) -> &str {
        self.name.as_str()
    }

    fn handle_irq(&self, cause: usize) {
        trace!("intc handle irq, cause {}", cause);
        self.with_handler(cause, |opt| {
            if let Some(h) = opt.get() {
                h();
            } else {
                warn!("no registered handler for SCAUSE {}!", cause);
            }
            Ok(())
        })
        .unwrap();
    }
}

impl IrqScheme for Intc {
    fn is_valid_irq(&self, cause: usize) -> bool {
        matches!(cause, S_SOFT | S_TIMER | S_EXT)
    }

    fn mask(&self, cause: usize) -> DeviceResult {
        unsafe {
            match cause {
                S_SOFT => sie::clear_ssoft(),
                S_TIMER => sie::clear_stimer(),
                S_EXT => sie::clear_sext(),
                _ => return Err(DeviceError::InvalidParam),
            }
        }
        Ok(())
    }

    fn unmask(&self, cause: usize) -> DeviceResult {
        unsafe {
            match cause {
                S_SOFT => sie::set_ssoft(),
                S_TIMER => sie::set_stimer(),
                S_EXT => sie::set_sext(),
                _ => return Err(DeviceError::InvalidParam),
            }
        }
        Ok(())
    }

    fn register_handler(&self, cause: usize, handler: IrqHandler) -> DeviceResult {
        self.with_handler(cause, |opt| {
            if opt.is_completed() {
                Err(DeviceError::AlreadyExists)
            } else {
                opt.call_once(|| handler);
                Ok(())
            }
        })
    }

    fn unregister(&self, _cause: usize) -> DeviceResult {
        panic!("unregister intc handler unsupported!");
    }
}

use riscv::register::sie;
use spin::Mutex;

use crate::prelude::IrqHandler;
use crate::scheme::{IrqScheme, Scheme};
use crate::{DeviceError, DeviceResult};

const S_SOFT: usize = 1;
const S_TIMER: usize = 5;
const S_EXT: usize = 9;

#[repr(usize)]
pub enum ScauseIntCode {
    SupervisorSoft = S_SOFT,
    SupervisorTimer = S_TIMER,
    SupervisorExternal = S_EXT,
}

pub struct Intc {
    soft_handler: Mutex<Option<IrqHandler>>,
    timer_handler: Mutex<Option<IrqHandler>>,
    ext_handler: Mutex<Option<IrqHandler>>,
}

impl Intc {
    pub fn new() -> Self {
        Self {
            soft_handler: Mutex::new(None),
            timer_handler: Mutex::new(None),
            ext_handler: Mutex::new(None),
        }
    }

    fn with_handler<F>(&self, cause: usize, op: F) -> DeviceResult
    where
        F: FnOnce(&mut Option<IrqHandler>) -> DeviceResult,
    {
        match cause {
            S_SOFT => op(&mut self.soft_handler.lock()),
            S_TIMER => op(&mut self.timer_handler.lock()),
            S_EXT => op(&mut self.ext_handler.lock()),
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
        "riscv-intc"
    }

    fn handle_irq(&self, cause: usize) {
        self.with_handler(cause, |opt| {
            if let Some(h) = opt {
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
            if opt.is_some() {
                Err(DeviceError::AlreadyExists)
            } else {
                *opt = Some(handler);
                Ok(())
            }
        })
    }

    fn unregister(&self, cause: usize) -> DeviceResult {
        self.with_handler(cause, |opt| {
            if opt.is_some() {
                *opt = None;
                Ok(())
            } else {
                Err(DeviceError::InvalidParam)
            }
        })
    }
}

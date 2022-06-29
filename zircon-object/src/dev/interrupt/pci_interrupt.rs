use alloc::{boxed::Box, sync::Arc};
use lock::Mutex;

use super::InterruptTrait;
use crate::dev::pci::{constants::PCIE_IRQRET_MASK, IPciNode};
use crate::{ZxError, ZxResult};

pub struct PciInterrupt {
    device: Arc<dyn IPciNode>,
    irq_id: usize,
    maskable: bool,
    inner: Mutex<PciInterruptInner>,
}

#[derive(Default)]
struct PciInterruptInner {
    register: bool,
}

impl PciInterrupt {
    pub fn new(device: Arc<dyn IPciNode>, vector: u32, maskable: bool) -> Box<Self> {
        // TODO check vector is a vaild IRQ number
        Box::new(PciInterrupt {
            device,
            irq_id: vector as _,
            maskable,
            inner: Default::default(),
        })
    }
}

impl InterruptTrait for PciInterrupt {
    fn mask(&self) {
        let inner = self.inner.lock();
        if self.maskable && inner.register {
            self.device.disable_irq(self.irq_id);
        }
    }

    fn unmask(&self) {
        let inner = self.inner.lock();
        if self.maskable && inner.register {
            self.device.enable_irq(self.irq_id);
        }
    }

    fn register_handler(&self, handle: Box<dyn Fn() + Send + Sync>) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.register {
            return Err(ZxError::ALREADY_BOUND);
        }
        self.device.register_irq_handle(
            self.irq_id,
            Box::new(move || {
                handle();
                PCIE_IRQRET_MASK
            }),
        );
        inner.register = true;
        Ok(())
    }

    fn unregister_handler(&self) -> ZxResult {
        let mut inner = self.inner.lock();
        if !inner.register {
            return Ok(());
        }
        self.device.unregister_irq_handle(self.irq_id);
        inner.register = false;
        Ok(())
    }
}

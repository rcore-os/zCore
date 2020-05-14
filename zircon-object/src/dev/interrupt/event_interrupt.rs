use {super::*, spin::Mutex};

pub struct EventInterrupt {
    vector: u8,
    inner: Mutex<EventInterruptInner>,
}

#[derive(Default)]
struct EventInterruptInner {
    register: bool,
}

impl EventInterrupt {
    pub fn new(vector: usize) -> Box<Self> {
        // TODO check vector is a vaild IRQ number
        Box::new(EventInterrupt {
            vector: vector as u8,
            inner: Default::default(),
        })
    }
}

impl InterruptTrait for EventInterrupt {
    fn mask(&self) {
        kernel_hal::irq_disable(self.vector as u8);
    }

    fn unmask(&self) {
        kernel_hal::irq_enable(self.vector as u8);
    }

    fn register_handler(&self, handle: Box<dyn Fn() + Send + Sync>) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.register {
            return Err(ZxError::ALREADY_BOUND);
        }
        if kernel_hal::irq_add_handle(self.vector, handle) {
            inner.register = true;
            Ok(())
        } else {
            Err(ZxError::ALREADY_BOUND)
        }
    }

    fn unregister_handler(&self) -> ZxResult {
        let mut inner = self.inner.lock();
        if !inner.register {
            return Ok(());
        }
        if kernel_hal::irq_remove_handle(self.vector) {
            inner.register = false;
            Ok(())
        } else {
            Err(ZxError::ALREADY_BOUND)
        } // maybe a better error code?
    }
}

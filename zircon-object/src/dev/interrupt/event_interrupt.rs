use kernel_hal::interrupt;
use {super::*, spin::Mutex};

pub struct EventInterrupt {
    vector: u32,
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
            vector: vector as u32,
            inner: Default::default(),
        })
    }
}

impl InterruptTrait for EventInterrupt {
    fn mask(&self) {
        let inner = self.inner.lock();
        if inner.register {
            interrupt::disable_irq(self.vector as u32);
        }
    }

    fn unmask(&self) {
        let inner = self.inner.lock();
        if inner.register {
            interrupt::enable_irq(self.vector as u32);
        }
    }

    fn register_handler(&self, handle: Box<dyn Fn() + Send + Sync>) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.register {
            return Err(ZxError::ALREADY_BOUND);
        }
        if interrupt::register_irq_handler(self.vector, handle).is_some() {
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
        if interrupt::unregister_irq_handler(self.vector) {
            inner.register = false;
            Ok(())
        } else {
            Err(ZxError::ALREADY_BOUND)
        } // maybe a better error code?
    }
}

use kernel_hal::InterruptManager;
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
            InterruptManager::disable(self.vector as u32);
        }
    }

    fn unmask(&self) {
        let inner = self.inner.lock();
        if inner.register {
            InterruptManager::enable(self.vector as u32);
        }
    }

    fn register_handler(&self, handle: Box<dyn Fn() + Send + Sync>) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.register {
            return Err(ZxError::ALREADY_BOUND);
        }
        if InterruptManager::set_ioapic_handle(self.vector, handle).is_some() {
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
        if InterruptManager::reset_ioapic_handle(self.vector) {
            inner.register = false;
            Ok(())
        } else {
            Err(ZxError::ALREADY_BOUND)
        } // maybe a better error code?
    }
}

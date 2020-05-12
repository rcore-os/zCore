use {
    super::*,
    alloc::sync::Arc,
    spin::Mutex,
};

pub struct EventInterrupt{
    vector: u8,
    inner: Mutex<EventInterruptInner>,
}

#[derive(Default)]
struct EventInterruptInner {
    register: bool,
}

impl EventInterrupt {
    pub fn new(vector: usize) -> Arc<Self> {
        // TODO check vector is a vaild IRQ number
        return Arc::new( EventInterrupt {
            vector: vector as u8,
            inner: Default::default(),
        });
    }
}

impl InterruptTrait for EventInterrupt {
    fn mask_interrupt_locked(&self) { kernel_hal::irq_disable(self.vector as u8); }
    fn unmask_interrupt_locked(&self) { kernel_hal::irq_enable(self.vector as u8); }
    fn register_interrupt_handler(&self, handle: Arc<dyn Fn() + Send + Sync>) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.register {
            return Err(ZxError::ALREADY_BOUND);
        }
        let result = kernel_hal::irq_add_handle(self.vector, handle);
        match result {
            true => {
                inner.register = true; 
                Ok(())
            },
            false => Err(ZxError::ALREADY_BOUND),
        }
    }
    fn unregister_interrupt_handler(&self) -> ZxResult {
        let mut inner = self.inner.lock();
        if !inner.register {
            return Ok(());
        }
        let result = kernel_hal::irq_remove_handle(self.vector);
        match result {
            true => {
                inner.register = false;
                Ok(())
            },
            false => Err(ZxError::ALREADY_BOUND), // maybe a better error code? 
        }
    }
}
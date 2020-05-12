use {
    super::*,
    alloc::sync::Arc,
};

pub struct EventInterrupt{
    vector: u8,
}


impl EventInterrupt {
    pub fn new(vector: usize) -> Arc<Self> {
        // TODO check vector is a vaild IRQ number
        return Arc::new(EventInterrupt {
            vector: vector as u8,
        });
    }
}

impl InterruptTrait for EventInterrupt {
    fn mask_interrupt_locked(&self) { kernel_hal::irq_disable(self.vector as u8); }
    fn unmask_interrupt_locked(&self) { kernel_hal::irq_enable(self.vector as u8); }
    fn register_interrupt_handler(&self, handle: Arc<dyn Fn() + Send + Sync>) -> ZxResult {
        let result = kernel_hal::irq_add_handle(self.vector, handle);
        match result {
            true => Ok(()),
            false => Err(ZxError::ALREADY_BOUND),
        }
    }
    fn unregister_interrupt_handler(&self) -> ZxResult {
        let result = kernel_hal::irq_remove_handle(self.vector);
        match result {
            true => Ok(()),
            false => Err(ZxError::ALREADY_BOUND), // maybe a better error code? 
        }
    }
}
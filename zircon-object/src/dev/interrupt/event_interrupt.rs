use {
    super::*,
    alloc::sync::Arc,
};

#[derive(Default)]
pub struct EventInterrupt{
    vector: u8,
}

impl EventInterrupt {
    pub fn new(vector: usize, f: fn()) -> Arc<Self> {
        // TODO check vector is a vaild IRQ number
        let event_interrupt =  EventInterrupt{ vector: vector as u8 };
        event_interrupt.register_interrupt_handler(f);
        event_interrupt.unmask_interrupt_locked();
        Arc::new(event_interrupt)
    }
}

impl InterruptTrait for EventInterrupt {
    fn mask_interrupt_locked(&self) { kernel_hal::irq_disable(self.vector as u8); }
    fn unmask_interrupt_locked(&self) { kernel_hal::irq_enable(self.vector as u8); }
    fn register_interrupt_handler(&self, handle: fn()) -> ZxResult {
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
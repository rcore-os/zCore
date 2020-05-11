use {
    super::*,
    alloc::sync::Arc,
};

#[derive(Default)]
pub struct EventInterrupt{
    vector: usize,
}

impl EventInterrupt {
    pub fn new(vector: usize) -> Arc<Self> {
        let event_interrupt =  EventInterrupt{ vector };
        event_interrupt.register_interrupt_handler();
        event_interrupt.unmask_interrupt_locked();
        Arc::new(event_interrupt)
    }

    pub fn register_int_handle(_vector: usize) {}
}

impl InterruptTrait for EventInterrupt {
    fn mask_interrupt_locked(&self) { kernel_hal::irq_disable(self.vector as u8); }
    fn unmask_interrupt_locked(&self) { kernel_hal::irq_enable(self.vector as u8); }
    fn register_interrupt_handler(&self) { Self::register_int_handle(self.vector); }
    fn unregister_interrupt_handler(&self) { Self::register_int_handle(self.vector); } 
}
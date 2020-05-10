use {
    super::*,
    alloc::sync::Arc,
};

#[derive(Default)]
pub struct VirtualInterrupt{
}

impl VirtualInterrupt {
    pub fn new() -> Arc<Self> {
        Default::default()
    }
}

impl InterruptTrait for VirtualInterrupt {
    fn mask_interrupt_locked(&self) {}
    fn unmask_interrupt_locked(&self) {}
    fn unregister_interrupt_handler(&self) {} 
}
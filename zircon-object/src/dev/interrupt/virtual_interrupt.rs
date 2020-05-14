use super::*;

#[derive(Default)]
pub struct VirtualInterrupt {}

impl VirtualInterrupt {
    pub fn new() -> Box<Self> {
        Default::default()
    }
}

impl InterruptTrait for VirtualInterrupt {
    fn mask(&self) {}
    fn unmask(&self) {}
    fn register_handler(&self, _handle: Box<dyn Fn() + Send + Sync>) -> ZxResult {
        Ok(())
    }
    fn unregister_handler(&self) -> ZxResult {
        Ok(())
    }
}

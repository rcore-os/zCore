use alloc::boxed::Box;

use super::Scheme;
use crate::DeviceResult;

pub type IrqHandler = Box<dyn Fn(usize) + Send + Sync>;

pub trait IrqScheme: Scheme {
    fn mask(&self, irq_num: usize);
    fn unmask(&self, irq_num: usize);

    fn register_handler(&self, irq_num: usize, handler: IrqHandler) -> DeviceResult;
    fn register_device(&self, irq_num: usize, dev: &'static dyn Scheme) -> DeviceResult {
        self.register_handler(irq_num, Box::new(move |n| dev.handle_irq(n)))
    }
}

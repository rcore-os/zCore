use core::ops::Range;

use crate::drivers::IRQ;
use crate::HalResult;

/*
lazy_static! {
    static ref MOUSE: Mutex<Mouse> = Mutex::new(Mouse::new());
    static ref MOUSE_CALLBACK: Mutex<Vec<Box<dyn Fn([u8; 3]) + Send + Sync>>> =
        Mutex::new(Vec::new());
}

#[export_name = "hal_mouse_set_callback"]
pub fn mouse_set_callback(callback: Box<dyn Fn([u8; 3]) + Send + Sync>) {
    MOUSE_CALLBACK.lock().push(callback);
}

fn mouse_on_complete(mouse_state: MouseState) {
    debug!("mouse state: {:?}", mouse_state);
    MOUSE_CALLBACK.lock().iter().for_each(|callback| {
        callback([
            mouse_state.get_flags().bits(),
            mouse_state.get_x() as u8,
            mouse_state.get_y() as u8,
        ]);
    });
}

fn mouse() {
    use x86_64::instructions::port::PortReadOnly;
    let mut port = PortReadOnly::new(0x60);
    let packet = unsafe { port.read() };
    MOUSE.lock().process_packet(packet);
}
*/

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {
            use x86_64::instructions::interrupts;
            interrupts::enable_and_hlt();
            interrupts::disable();
        }

        fn is_valid_irq(gsi: usize) -> bool {
            IRQ.is_valid_irq(gsi)
        }

        fn mask_irq(gsi: usize) -> HalResult {
            Ok(IRQ.mask(gsi)?)
        }

        fn unmask_irq(gsi: usize) -> HalResult {
            Ok(IRQ.unmask(gsi)?)
        }

        fn configure_irq(gsi: usize, tm: IrqTriggerMode, pol: IrqPolarity) -> HalResult {
            Ok(IRQ.configure(gsi, tm, pol)?)
        }

        fn register_irq_handler(gsi: usize, handler: IrqHandler) -> HalResult {
            Ok(IRQ.register_handler(gsi, handler)?)
        }

        fn unregister_irq_handler(gsi: usize) -> HalResult {
            Ok(IRQ.unregister(gsi)?)
        }

        fn handle_irq(vector: usize) {
            IRQ.handle_irq(vector as usize);
        }

        fn msi_alloc_block(requested_irqs: usize) -> HalResult<Range<usize>> {
            Ok(IRQ.msi_alloc_block(requested_irqs)?)
        }

        fn msi_free_block(block: Range<usize>) -> HalResult {
            Ok(IRQ.msi_free_block(block)?)
        }

        fn msi_register_handler(
            block: Range<usize>,
            msi_id: usize,
            handler: IrqHandler,
        ) -> HalResult {
            Ok(IRQ.msi_register_handler(block, msi_id, handler)?)
        }
    }
}

/*
fn keyboard() {
    use pc_keyboard::{DecodedKey, KeyCode};
    if let Some(key) = super::keyboard::receive() {
        match key {
            DecodedKey::Unicode(c) => super::serial_put(c as u8),
            DecodedKey::RawKey(code) => {
                let s = match code {
                    KeyCode::ArrowUp => "\u{1b}[A",
                    KeyCode::ArrowDown => "\u{1b}[B",
                    KeyCode::ArrowRight => "\u{1b}[C",
                    KeyCode::ArrowLeft => "\u{1b}[D",
                    _ => "",
                };
                for c in s.bytes() {
                    super::serial_put(c);
                }
            }
        }
    }
}
*/

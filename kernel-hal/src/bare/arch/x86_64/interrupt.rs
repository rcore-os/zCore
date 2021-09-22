use alloc::boxed::Box;
use core::ops::Range;

use crate::HalResult;

type IrqHandler = Box<dyn Fn() + Send + Sync>;

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
        fn enable_irq(irq: u32) {
            todo!()
        }

        fn disable_irq(irq: u32) {
            todo!()
        }

        fn is_valid_irq(irq: u32) -> bool {
            todo!()
        }

        fn configure_irq(vector: u32, trig_mode: bool, polarity: bool) -> HalResult {
            todo!()
        }

        fn register_irq_handler(global_irq: u32, handler: IrqHandler) -> HalResult<u32> {
            todo!()
        }

        fn unregister_irq_handler(global_irq: u32) -> HalResult {
            todo!()
        }

        fn handle_irq(vector: u32) {
            crate::drivers::IRQ.handle_irq(vector as usize);
        }

        fn msi_allocate_block(requested_irqs: u32) -> HalResult<Range<u32>> {
            todo!()
        }

        fn msi_free_block(block: Range<u32>) -> HalResult {
            todo!()
        }

        fn msi_register_handler(
            block: Range<u32>,
            msi_id: u32,
            handler: Box<dyn Fn() + Send + Sync>,
        ) -> HalResult {
            todo!()
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

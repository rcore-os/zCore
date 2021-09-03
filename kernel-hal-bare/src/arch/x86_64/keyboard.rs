use alloc::boxed::Box;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1};
use spin::Mutex;
use x86_64::instructions::port::Port;

lazy_static! {
    static ref KEYBOARD: Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>> = Mutex::new(
        Keyboard::new(layouts::Us104Key, ScancodeSet1, HandleControl::Ignore)
    );
    static ref KBD_CALLBACK: Mutex<Vec<Box<dyn Fn(u16, i32) + Send + Sync>>> =
        Mutex::new(Vec::new());
}

#[export_name = "hal_kbd_set_callback"]
pub fn kbd_set_callback(callback: Box<dyn Fn(u16, i32) + Send + Sync>) {
    KBD_CALLBACK.lock().push(callback);
}

/// Receive character from keyboard
/// Should be called on every interrupt
pub fn receive() -> Option<DecodedKey> {
    let mut keyboard = KEYBOARD.lock();
    let mut data_port = Port::<u8>::new(0x60);
    let mut status_port = Port::<u8>::new(0x64);

    // Output buffer status = 1
    if unsafe { status_port.read() } & 1 != 0 {
        let scancode = unsafe { data_port.read() };
        KBD_CALLBACK.lock().iter().for_each(|callback| {
            match scancode {
                0x80..=0xFF => callback((scancode as u16) - 0x80, 0),
                _ => callback(scancode as u16, 1),
            };
        });
        if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
            return keyboard.process_keyevent(key_event);
        }
    }
    None
}

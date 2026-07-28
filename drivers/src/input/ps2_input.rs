use lock::Mutex;
use x86_64::instructions::port::Port;

use crate::prelude::{CapabilityType, InputCapability, InputEvent, InputEventType};
use crate::scheme::{impl_event_scheme, InputScheme, Scheme};
use crate::utils::EventListener;

pub struct Ps2Input {
    listener: EventListener<InputEvent>,
    extended: Mutex<bool>,
    mouse_state: Mutex<MouseState>,
}

#[derive(Default)]
struct MouseState {
    phase: u8,
    bytes: [u8; 3],
}

fn wait_write() {
    let mut status_port = Port::<u8>::new(0x64);
    let mut timeout = 100_000;
    unsafe {
        while (status_port.read() & 0x02) != 0 && timeout > 0 {
            timeout -= 1;
        }
    }
}

fn wait_read() -> bool {
    let mut status_port = Port::<u8>::new(0x64);
    let mut timeout = 100_000;
    unsafe {
        while (status_port.read() & 0x01) == 0 && timeout > 0 {
            timeout -= 1;
        }
        timeout > 0
    }
}

impl Default for Ps2Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Ps2Input {
    pub fn new() -> Self {
        // Initialize PS/2 controller
        unsafe {
            let mut data_port = Port::<u8>::new(0x60);
            let mut status_port = Port::<u8>::new(0x64);

            // 1. Enable ports (keyboard & mouse)
            wait_write();
            status_port.write(0xAE); // Enable keyboard port
            wait_write();
            status_port.write(0xA8); // Enable mouse port

            // 2. Read Controller Configuration Byte
            wait_write();
            status_port.write(0x20);
            let mut config = 0;
            if wait_read() {
                config = data_port.read();
            }

            // 3. Update Configuration Byte:
            // - Bit 0: Enable keyboard interrupt
            // - Bit 1: Enable mouse interrupt
            // - Bit 4: Clear disable keyboard clock
            // - Bit 5: Clear disable mouse clock
            // - Bit 6: Enable translation to Scan Code Set 1
            config |= 0x01;
            config |= 0x02;
            config &= !0x10;
            config &= !0x20;
            config |= 0x40;

            wait_write();
            status_port.write(0x60);
            wait_write();
            data_port.write(config);

            // 4. Reset & enable mouse data reporting
            // Write D4 to status port (direct next write to aux device)
            wait_write();
            status_port.write(0xD4);
            wait_write();
            data_port.write(0xF4); // Enable packet reporting command

            // Wait for mouse ACK (0xFA)
            if wait_read() {
                let _ack = data_port.read();
            }
        }

        Self {
            listener: EventListener::new(),
            extended: Mutex::new(false),
            mouse_state: Mutex::new(MouseState::default()),
        }
    }
}

impl_event_scheme!(Ps2Input, InputEvent);

impl Scheme for Ps2Input {
    fn name(&self) -> &str {
        "ps2-input"
    }

    fn handle_irq(&self, _irq_num: usize) {
        let mut data_port = Port::<u8>::new(0x60);
        let mut status_port = Port::<u8>::new(0x64);

        unsafe {
            loop {
                let status = status_port.read();
                if (status & 0x01) == 0 {
                    break;
                }

                let is_aux = (status & 0x20) != 0;
                let code = data_port.read();

                if is_aux {
                    // Handle mouse data
                    let mut state = self.mouse_state.lock();
                    let phase = state.phase as usize;
                    state.bytes[phase] = code;
                    state.phase += 1;

                    if state.phase == 3 {
                        let flags = state.bytes[0];
                        let dx_raw = state.bytes[1];
                        let dy_raw = state.bytes[2];
                        state.phase = 0;

                        // Check signature bit (bit 3 of first byte should be 1)
                        if (flags & 0x08) != 0 {
                            // Translate relative coordinates
                            let x_neg = (flags & 0x10) != 0;
                            let y_neg = (flags & 0x20) != 0;

                            let dx = if x_neg {
                                (dx_raw as i16 - 256) as i32
                            } else {
                                dx_raw as i32
                            };

                            let dy = if y_neg {
                                (dy_raw as i16 - 256) as i32
                            } else {
                                dy_raw as i32
                            };

                            // RelAxis X/Y
                            self.listener.trigger(InputEvent {
                                event_type: InputEventType::RelAxis,
                                code: 0, // REL_X
                                value: dx,
                            });
                            self.listener.trigger(InputEvent {
                                event_type: InputEventType::RelAxis,
                                code: 1,    // REL_Y
                                value: -dy, // Invert Y delta for standard mouse behavior
                            });

                            // Buttons
                            let left = (flags & 0x01) != 0;
                            let right = (flags & 0x02) != 0;
                            let middle = (flags & 0x04) != 0;

                            self.listener.trigger(InputEvent {
                                event_type: InputEventType::Key,
                                code: 0x110, // BTN_LEFT
                                value: if left { 1 } else { 0 },
                            });
                            self.listener.trigger(InputEvent {
                                event_type: InputEventType::Key,
                                code: 0x111, // BTN_RIGHT
                                value: if right { 1 } else { 0 },
                            });
                            self.listener.trigger(InputEvent {
                                event_type: InputEventType::Key,
                                code: 0x112, // BTN_MIDDLE
                                value: if middle { 1 } else { 0 },
                            });

                            // Sync
                            self.listener.trigger(InputEvent {
                                event_type: InputEventType::Syn,
                                code: 0,
                                value: 0,
                            });
                        }
                    }
                } else {
                    // Handle keyboard data
                    if code == 0xE0 {
                        *self.extended.lock() = true;
                        continue;
                    }

                    let is_extended = {
                        let mut ext = self.extended.lock();
                        let was_ext = *ext;
                        *ext = false;
                        was_ext
                    };

                    let pressed = (code & 0x80) == 0;
                    let scancode = code & 0x7F;

                    let keycode = if is_extended {
                        match scancode {
                            0x48 => 103, // Up
                            0x50 => 108, // Down
                            0x4B => 105, // Left
                            0x4D => 106, // Right
                            0x1D => 97,  // RCtrl
                            0x38 => 100, // RAlt / AltGr
                            0x35 => 98,  // KP_Divide
                            0x1C => 96,  // KP_Enter
                            0x53 => 111, // Delete
                            _ => scancode as u16,
                        }
                    } else {
                        scancode as u16
                    };

                    self.listener.trigger(InputEvent {
                        event_type: InputEventType::Key,
                        code: keycode,
                        value: if pressed { 1 } else { 0 },
                    });

                    self.listener.trigger(InputEvent {
                        event_type: InputEventType::Syn,
                        code: 0,
                        value: 0,
                    });
                }
            }
        }
    }
}

impl InputScheme for Ps2Input {
    fn capability(&self, cap_type: CapabilityType) -> InputCapability {
        let mut cap = InputCapability::empty();
        match cap_type {
            CapabilityType::Event => {
                cap.set(crate::input::input_event_codes::ev::EV_SYN);
                cap.set(crate::input::input_event_codes::ev::EV_KEY);
                cap.set(crate::input::input_event_codes::ev::EV_REL);
            }
            CapabilityType::Key => {
                for i in 0..0x120 {
                    cap.set(i);
                }
            }
            CapabilityType::RelAxis => {
                cap.set(0); // REL_X
                cap.set(1); // REL_Y
            }
            _ => {}
        }
        cap
    }
}

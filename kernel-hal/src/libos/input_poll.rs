//! libOS host build: HID poll is a no-op (real xHCI path is bare-metal only).

/// Poll USB/HID devices (no-op on libOS).
pub fn poll_input_devices() {}

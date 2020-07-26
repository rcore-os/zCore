//! Input/output for mipsel.
use crate::drivers::SERIAL_DRIVERS;
use core::fmt::Arguments;

pub fn putfmt(fmt: Arguments) {
    let mut drivers = SERIAL_DRIVERS.write();
    if let Some(serial) = drivers.first_mut() {
        serial.write(format!("{}", fmt).as_bytes());
    }
}

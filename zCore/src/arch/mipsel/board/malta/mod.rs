/// Device tree bytes
pub static DTB: &'static [u8] = include_bytes!("device.dtb");

/// Initialize other board drivers
pub fn init(dtb: usize) {
    // TODO: add possibly more drivers
    kernel_hal_bare::drivers::serial::uart16550::driver_init();
    kernel_hal_bare::drivers::device_tree::init(dtb);
}

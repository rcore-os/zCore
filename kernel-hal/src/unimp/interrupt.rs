use alloc::boxed::Box;

/// Enable IRQ.
pub fn enable_irq(_vector: u32) {
    unimplemented!()
}

/// Disable IRQ.
pub fn disable_irq(_vector: u32) {
    unimplemented!()
}

/// Is a valid IRQ number.
pub fn is_valid_irq(_vector: u32) -> bool {
    unimplemented!()
}

/// Configure the specified interrupt vector.  If it is invoked, it muust be
/// invoked prior to interrupt registration.
pub fn configure_irq(_vector: u32, _trig_mode: bool, _polarity: bool) -> bool {
    unimplemented!()
}

/// Add an interrupt handle to an IRQ
pub fn register_irq_handler(_vector: u32, _handle: Box<dyn Fn() + Send + Sync>) -> Option<u32> {
    unimplemented!()
}

/// Remove the interrupt handle to an IRQ
pub fn unregister_irq_handler(_vector: u32) -> bool {
    unimplemented!()
}

/// Handle IRQ.
pub fn handle_irq(_vector: u32) {
    unimplemented!()
}

/// Method used for platform allocation of blocks of MSI and MSI-X compatible
/// IRQ targets.
pub fn msi_allocate_block(_irq_num: u32) -> Option<(usize, usize)> {
    unimplemented!()
}

/// Method used to free a block of MSI IRQs previously allocated by msi_alloc_block().
/// This does not unregister IRQ handlers.
pub fn msi_free_block(_irq_start: u32, _irq_num: u32) {
    unimplemented!()
}

/// Register a handler function for a given msi_id within an msi_block_t. Passing a
/// NULL handler will effectively unregister a handler for a given msi_id within the
/// block.
pub fn msi_register_handler(
    _irq_start: u32,
    _irq_num: u32,
    _msi_id: u32,
    _handle: Box<dyn Fn() + Send + Sync>,
) {
    unimplemented!()
}

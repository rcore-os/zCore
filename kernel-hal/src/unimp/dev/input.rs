use alloc::boxed::Box;

pub fn kbd_set_callback(_callback: Box<dyn Fn(u16, i32) + Send + Sync>) {
    unimplemented!()
}

pub fn mice_set_callback(_callback: Box<dyn Fn([u8; 3]) + Send + Sync>) {
    unimplemented!()
}

pub fn init() {
    unimplemented!()
}

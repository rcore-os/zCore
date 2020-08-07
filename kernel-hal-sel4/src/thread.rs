use crate::sys;

pub fn yield_now() {
    unsafe {
        sys::l4bridge_yield();
    }
}

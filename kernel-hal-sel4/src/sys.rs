use crate::types::*;
use crate::sync::YieldMutex;

#[link(name = "zc_loader", kind = "static")]
extern "C" {
    pub fn l4bridge_putchar(c: u8);
    pub fn l4bridge_yield();
    pub fn l4bridge_alloc_frame(slot: CPtr, paddr_out: &mut Word) -> i32;
    pub fn l4bridge_delete_cap(slot: CPtr);
    pub fn l4bridge_ensure_cslot(slot: CPtr) -> i32;
}

static M: YieldMutex<()> = YieldMutex::new(());

pub fn locked<F: FnOnce() -> R, R>(f: F) -> R {
    let _guard = M.lock();
    let ret = f();
    ret
}

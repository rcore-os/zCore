use crate::types::*;

#[link(name = "zc_loader", kind = "static")]
extern "C" {
    pub fn l4bridge_putchar(c: u8);
    pub fn l4bridge_yield();
    pub fn l4bridge_alloc_frame(slot: CPtr, paddr_out: &mut Word) -> i32;
    pub fn l4bridge_delete_cap(slot: CPtr);
}

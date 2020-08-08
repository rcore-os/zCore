use crate::types::*;
use crate::sync::YieldMutex;

#[link(name = "zc_loader", kind = "static")]
extern "C" {
    pub fn l4bridge_putchar(c: u8);
    pub fn l4bridge_yield();
    pub fn l4bridge_alloc_untyped(slot: CPtr, bits: i32, paddr_out: &mut Word) -> i32;
    pub fn l4bridge_split_untyped(src: CPtr, src_bits: i32, dst0: CPtr, dst1: CPtr) -> i32;
    pub fn l4bridge_retype_and_mount_cnode(slot: CPtr, num_slots_bits: i32, target_index: Word) -> i32;
    pub fn l4bridge_delete_cap(slot: CPtr);

    pub static L4BRIDGE_CNODE_SLOT_BITS: Word;
}

static M: YieldMutex<()> = YieldMutex::new(());

pub fn locked<F: FnOnce() -> R, R>(f: F) -> R {
    let _guard = M.lock();
    let ret = f();
    ret
}

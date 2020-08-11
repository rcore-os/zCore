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

    pub fn l4bridge_retype_pdpt(untyped: CPtr, out: CPtr) -> i32;
    pub fn l4bridge_retype_pagedir(untyped: CPtr, out: CPtr) -> i32;
    pub fn l4bridge_retype_pagetable(untyped: CPtr, out: CPtr) -> i32;
    pub fn l4bridge_retype_page(untyped: CPtr, out: CPtr) -> i32;
    pub fn l4bridge_retype_tcb(untyped: CPtr, out: CPtr) -> i32;
    pub fn l4bridge_retype_endpoint(untyped: CPtr, out: CPtr) -> i32;

    pub fn l4bridge_map_pdpt(slot: CPtr, vspace: CPtr, vaddr: Word) -> i32;
    pub fn l4bridge_map_pagedir(slot: CPtr, vspace: CPtr, vaddr: Word) -> i32;
    pub fn l4bridge_map_pagetable(slot: CPtr, vspace: CPtr, vaddr: Word) -> i32;
    pub fn l4bridge_map_page(slot: CPtr, vspace: CPtr, vaddr: Word, attributes: i32) -> i32;

    pub fn l4bridge_configure_tcb(
        tcb: CPtr,
        fault_ep: CPtr,
        cspace_root: CPtr, vspace_root: CPtr,
        ipc_buffer: Word, ipc_buffer_frame: CPtr,
    ) -> i32;

    pub fn l4bridge_set_priority(
        tcb: CPtr,
        auth_tcb: CPtr,
        priority: Word,
    ) -> i32;

    pub fn l4bridge_set_pc_sp(
        tcb: CPtr,
        pc: Word,
        sp: Word
    ) -> i32;

    pub fn l4bridge_get_pc_sp(
        tcb: CPtr,
        pc: &mut Word,
        sp: &mut Word
    ) -> i32;

    pub fn l4bridge_resume(
        tcb: CPtr
    ) -> i32;

    pub fn l4bridge_setup_tls(
        tls_addr: Word,
        tls_size: Word,
        ipc_buffer: Word,
    );

    pub fn l4bridge_badge_endpoint(src: CPtr, dst: CPtr, badge: Word) -> i32;

    pub fn l4bridge_kipc_call(slot: CPtr, data: Word, result: &mut Word) -> i32;
    pub fn l4bridge_kipc_recv(slot: CPtr, data: &mut Word, sender_badge: &mut Word) -> i32;
    pub fn l4bridge_kipc_reply(result: Word);

    pub static L4BRIDGE_CNODE_SLOT_BITS: Word;
    pub static L4BRIDGE_TCB_BITS: Word;
    pub static L4BRIDGE_STATIC_CAP_VSPACE: Word;
    pub static L4BRIDGE_STATIC_CAP_CSPACE: Word;
    pub static L4BRIDGE_STATIC_CAP_TCB: Word;
    pub static L4BRIDGE_PDPT_BITS: Word;
    pub static L4BRIDGE_PAGEDIR_BITS: Word;
    pub static L4BRIDGE_PAGETABLE_BITS: Word;
    pub static L4BRIDGE_PAGE_BITS: Word;
    pub static L4BRIDGE_MAX_PRIO: Word;
    pub static L4BRIDGE_ENDPOINT_BITS: Word;
}

static M: YieldMutex<()> = YieldMutex::new(());

pub fn locked<F: FnOnce() -> R, R>(f: F) -> R {
    let _guard = M.lock();
    let ret = f();
    ret
}

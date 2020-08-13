#include <sel4/sel4.h>

#define CNODE_SLOT 1
#define FAULT_ENDPOINT_SLOT 2
#define PD_SLOT 3
#define ASID_POOL_SLOT 4
#define TCB_SLOT 5

#define GETCAP_CPTR 8

#define TEMP_CPTR 63
#define NEW_ROOT_CNODE_CPTR 62
#define RETYPE_BUF_1_CPTR 61
#define RETYPE_BUF_0_CPTR 60

#define ZCDAEMON_IPCBUF_VADDR 0x3000000
#define TOPLEVEL_CNODE_BITS 12
#define SECONDLEVEL_CNODE_BITS 12

#ifndef MASK
#define MASK(n) (BIT(n)-1ul)
#endif

#ifndef BIT
#define BIT(n) (1ul<<(n))
#endif

// TLS
// FIXME: Actually load TLS.
#define TLS_SIZE 8192
static char TLS[TLS_SIZE];

// IPC buffer
static seL4_IPCBuffer *ipc_buffer = (seL4_IPCBuffer *) ZCDAEMON_IPCBUF_VADDR;

// Thread-local context.
static __thread void *thread_local_context;

extern void rust_start();

// Fails if not stripped before linking.
void static_assert();

seL4_CPtr putchar_cptr = 0;
seL4_CPtr alloc_untyped_cptr = 0;
seL4_CPtr alloc_cnode_cptr = 0;
seL4_CPtr timer_event_cptr = 0;
seL4_CPtr set_period_cptr = 0;
seL4_CPtr get_time_cptr = 0;
seL4_CPtr asid_control_cptr = 0;

seL4_Word L4BRIDGE_CNODE_SLOT_BITS = seL4_SlotBits;
seL4_Word L4BRIDGE_TCB_BITS = seL4_TCBBits;
seL4_Word L4BRIDGE_STATIC_CAP_VSPACE = PD_SLOT;
seL4_Word L4BRIDGE_STATIC_CAP_CSPACE = CNODE_SLOT;
seL4_Word L4BRIDGE_STATIC_CAP_TCB = TCB_SLOT;
seL4_Word L4BRIDGE_VSPACE_BITS = seL4_PML4Bits;
seL4_Word L4BRIDGE_PDPT_BITS = seL4_PDPTBits;
seL4_Word L4BRIDGE_PAGEDIR_BITS = seL4_PageDirBits;
seL4_Word L4BRIDGE_PAGETABLE_BITS = seL4_PageTableBits;
seL4_Word L4BRIDGE_PAGE_BITS = seL4_PageBits;
seL4_Word L4BRIDGE_ENDPOINT_BITS = seL4_EndpointBits;
seL4_Word L4BRIDGE_MAX_PRIO = seL4_MaxPrio;
seL4_Word L4BRIDGE_NUM_REGISTERS = sizeof(seL4_UserContext) / sizeof(seL4_Word);
seL4_Word L4BRIDGE_FAULT_UNKNOWN_SYSCALL = seL4_Fault_UnknownSyscall;
seL4_Word L4BRIDGE_FAULT_VM = seL4_Fault_VMFault;
seL4_Word L4BRIDGE_ASID_POOL_BITS = 12; // 4K
seL4_Word L4BRIDGE_ENTRIES_PER_ASID_POOL = 1024;

char fmt_hex_char(unsigned char v) {
    if(v >= 0 && v <= 9) {
        return '0' + v;
    } else {
        return 'a' + (v - 10);
    }
}

void fmt_word(char out[18], seL4_Word w) {
    unsigned char *raw = (unsigned char *) &w;
    for(int i = 0; i < 8; i++) {
        out[i * 2] = fmt_hex_char((raw[8 - 1 - i] & 0xf0) >> 4);
        out[i * 2 + 1] = fmt_hex_char(raw[8 - 1 - i] & 0xf);
    }
    out[16] = '\n';
    out[17] = 0;
}

static void set_tls_base(seL4_Word x) {
#ifdef CONFIG_SET_TLS_BASE_SELF
    seL4_SetTLSBase(x);
#else
    asm volatile("wrfsbase %0" :: "r"(x));
#endif
}

void init_master_tls() {
    // reference: https://wiki.osdev.org/Thread_Local_Storage
    seL4_Word thread_area = (seL4_Word) TLS + TLS_SIZE - 0x1000;
    * (seL4_Word *) thread_area = thread_area;
    set_tls_base(thread_area);
}

void write_string_buf(char *dst, const char *src, int dst_size) {
    for(int i = 0; i < dst_size; i++) {
        dst[i] = src[i];
        if(src[i] == 0) return;
    }
    dst[dst_size - 1] = 0;
}

void l4bridge_putchar(char c) {
    seL4_SetMR(0, c);
    seL4_MessageInfo_t tag = seL4_MessageInfo_new(0, 0, 0, 1);
    seL4_Call(putchar_cptr, tag);
}

void l4bridge_setup_tls(seL4_Word tls_addr, seL4_Word tls_size, seL4_Word ipc_buffer) {
    // reference: https://wiki.osdev.org/Thread_Local_Storage
    seL4_Word thread_area = (seL4_Word) tls_addr + tls_size - 0x1000;
    * (seL4_Word *) thread_area = thread_area;
    set_tls_base(thread_area);
    seL4_SetIPCBuffer((seL4_IPCBuffer *) ipc_buffer);
}

void ** l4bridge_get_thread_local_context() {
    return &thread_local_context;
}

void print_str(const char *s) {
    while(*s) {
        l4bridge_putchar(*s);
        s++;
    }
}

void panic_str(const char *s) {
    print_str(s);
    print_str("[loader] PANIC.\n");
    while(1);
}

void print_word(seL4_Word word) {
    char buf[18];
    fmt_word(buf, word);
    print_str(buf);
}

static int alloc_cnode(seL4_CPtr slot, int bits) {
    seL4_SetCapReceivePath(CNODE_SLOT, slot, seL4_WordBits);
    seL4_SetMR(0, bits);
    seL4_MessageInfo_t tag = seL4_Call(alloc_cnode_cptr, seL4_MessageInfo_new(0, 0, 0, 1));
    seL4_SetCapReceivePath(0, 0, 0);

    if(
        seL4_MessageInfo_get_label(tag) != 0 ||
        seL4_MessageInfo_get_extraCaps(tag) != 1 ||
        seL4_MessageInfo_get_length(tag) != 0
    ) {
        return 1;
    }
    return 0;
}

void l4bridge_yield() {
    seL4_Yield();
}

int l4bridge_alloc_untyped(seL4_CPtr slot, int bits, seL4_Word *paddr_out) {
    seL4_SetCapReceivePath(CNODE_SLOT, slot, seL4_WordBits);
    seL4_SetMR(0, bits);
    seL4_MessageInfo_t tag = seL4_Call(alloc_untyped_cptr, seL4_MessageInfo_new(0, 0, 0, 1));
    seL4_SetCapReceivePath(0, 0, 0);

    if(
        seL4_MessageInfo_get_label(tag) != 0 ||
        seL4_MessageInfo_get_extraCaps(tag) != 1 ||
        seL4_MessageInfo_get_length(tag) != 1
    ) {
        return 1;
    }
    *paddr_out = seL4_GetMR(0);
    return 0;
}

int l4bridge_split_untyped(seL4_CPtr src, int src_bits, seL4_CPtr dst0, seL4_CPtr dst1) {
    int error;
    
    error = seL4_Untyped_Retype(
        src, seL4_UntypedObject, src_bits - 1,
        CNODE_SLOT, 0, seL4_WordBits - SECONDLEVEL_CNODE_BITS,
        RETYPE_BUF_0_CPTR, 2
    );
    if(error) return error;

    error = seL4_CNode_Move(
        CNODE_SLOT, dst0, seL4_WordBits,
        CNODE_SLOT, RETYPE_BUF_0_CPTR, seL4_WordBits
    );
    if(error) return error;

    error = seL4_CNode_Move(
        CNODE_SLOT, dst1, seL4_WordBits,
        CNODE_SLOT, RETYPE_BUF_1_CPTR, seL4_WordBits
    );
    if(error) return error;

    return 0;
}

int l4bridge_retype_and_mount_cnode(seL4_CPtr slot, int num_slots_bits, seL4_Word target_index) {
    int error;
    
    error = seL4_Untyped_Retype(
        slot, seL4_CapTableObject, num_slots_bits,
        CNODE_SLOT, 0, seL4_WordBits - SECONDLEVEL_CNODE_BITS,
        RETYPE_BUF_0_CPTR, 1
    );
    if(error) return error;

    error = seL4_CNode_Mutate(
        CNODE_SLOT, target_index, seL4_WordBits - SECONDLEVEL_CNODE_BITS,
        CNODE_SLOT, RETYPE_BUF_0_CPTR, seL4_WordBits,
        seL4_CNode_CapData_new(0, 0).words[0]
    );
    if(error) return error;

    return 0;
}

static int _l4bridge_retype_object(
    seL4_CPtr untyped,
    seL4_CPtr out,
    seL4_Word dst_type,
    seL4_Word size_bits
) {
    int error;

    error = seL4_Untyped_Retype(
        untyped, dst_type, size_bits,
        CNODE_SLOT, 0, seL4_WordBits - SECONDLEVEL_CNODE_BITS,
        TEMP_CPTR, 1
    );
    if(error) return error;

    error = seL4_CNode_Move(
        CNODE_SLOT, out, seL4_WordBits,
        CNODE_SLOT, TEMP_CPTR, seL4_WordBits
    );
    if(error) return error;

    return 0;
}

static int _l4bridge_retype_fixed_size_object(
    seL4_CPtr untyped,
    seL4_CPtr out,
    seL4_Word dst_type
) {
    return _l4bridge_retype_object(untyped, out, dst_type, 0);
}

int l4bridge_retype_vspace(
    seL4_CPtr untyped,
    seL4_CPtr out
) {
    return _l4bridge_retype_fixed_size_object(untyped, out, seL4_X64_PML4Object);
}

int l4bridge_retype_pdpt(
    seL4_CPtr untyped,
    seL4_CPtr out
) {
    return _l4bridge_retype_fixed_size_object(untyped, out, seL4_X86_PDPTObject);
}

int l4bridge_retype_pagedir(
    seL4_CPtr untyped,
    seL4_CPtr out
) {
    return _l4bridge_retype_fixed_size_object(untyped, out, seL4_X86_PageDirectoryObject);
}

int l4bridge_retype_pagetable(
    seL4_CPtr untyped,
    seL4_CPtr out
) {
    return _l4bridge_retype_fixed_size_object(untyped, out, seL4_X86_PageTableObject);
}

int l4bridge_retype_page(
    seL4_CPtr untyped,
    seL4_CPtr out
) {
    return _l4bridge_retype_fixed_size_object(untyped, out, seL4_X86_4K);
}

int l4bridge_retype_tcb(
    seL4_CPtr untyped,
    seL4_CPtr out
) {
    return _l4bridge_retype_fixed_size_object(untyped, out, seL4_TCBObject);
}

int l4bridge_retype_endpoint(
    seL4_CPtr untyped,
    seL4_CPtr out
) {
    return _l4bridge_retype_fixed_size_object(untyped, out, seL4_EndpointObject);
}

int l4bridge_retype_cnode(
    seL4_CPtr untyped,
    seL4_CPtr out,
    seL4_Word size_bits
) {
    return _l4bridge_retype_object(untyped, out, seL4_CapTableObject, size_bits);
}

int l4bridge_map_pdpt(
    seL4_CPtr slot,
    seL4_CPtr vspace,
    seL4_Word vaddr
) {
    return seL4_X86_PDPT_Map(
        slot, vspace, vaddr, seL4_X86_Default_VMAttributes
    );
}

int l4bridge_map_pagedir(
    seL4_CPtr slot,
    seL4_CPtr vspace,
    seL4_Word vaddr
) {
    return seL4_X86_PageDirectory_Map(
        slot, vspace, vaddr, seL4_X86_Default_VMAttributes
    );
}

int l4bridge_map_pagetable(
    seL4_CPtr slot,
    seL4_CPtr vspace,
    seL4_Word vaddr
) {
    return seL4_X86_PageTable_Map(
        slot, vspace, vaddr, seL4_X86_Default_VMAttributes
    );
}

int l4bridge_map_page(
    seL4_CPtr slot,
    seL4_CPtr vspace,
    seL4_Word vaddr,
    int attributes
) {
    return seL4_X86_Page_Map(
        slot, vspace, vaddr, seL4_AllRights, seL4_X86_Default_VMAttributes
    );
}

int l4bridge_configure_tcb(
    seL4_CPtr tcb,
    seL4_CPtr fault_ep,
    seL4_CPtr cspace_root,
    seL4_CPtr vspace_root,
    seL4_Word ipc_buffer,
    seL4_CPtr ipc_buffer_frame
) {
    return seL4_TCB_Configure(tcb, fault_ep, cspace_root, 0, vspace_root, 0, ipc_buffer, ipc_buffer_frame);
}

int l4bridge_set_priority(
    seL4_CPtr tcb,
    seL4_CPtr auth_tcb,
    seL4_Word priority
) {
    return seL4_TCB_SetPriority(tcb, auth_tcb, priority);
}

int l4bridge_set_pc_sp(
    seL4_CPtr tcb,
    seL4_Word pc,
    seL4_Word sp
) {
    seL4_UserContext ctx = {0};
    ctx.rip = pc;
    ctx.rsp = sp;
    return seL4_TCB_WriteRegisters(tcb, 0, 0, 2, &ctx);
}

int l4bridge_get_pc_sp(
    seL4_CPtr tcb,
    seL4_Word *pc,
    seL4_Word *sp
) {
    seL4_UserContext ctx = {0};
    int error = seL4_TCB_ReadRegisters(tcb, 0, 0, 2, &ctx);
    if(error) return error;
    *pc = ctx.rip;
    *sp = ctx.rsp;
    return 0;
}

int l4bridge_write_all_registers_ts(
    seL4_CPtr tcb,
    const seL4_UserContext *regs,
    int resume
) {
    return seL4_TCB_WriteRegisters(tcb, resume, 0, L4BRIDGE_NUM_REGISTERS, (seL4_UserContext *) regs);
}

int l4bridge_read_all_registers_ts(
    seL4_CPtr tcb,
    seL4_UserContext *regs,
    int suspend
) {
    return seL4_TCB_ReadRegisters(tcb, suspend, 0, L4BRIDGE_NUM_REGISTERS, regs);
}

static int handle_fault_ipc_reentry_generic(seL4_MessageInfo_t tag, seL4_UserContext *regs) {
    return seL4_MessageInfo_get_label(tag);
}

int l4bridge_fault_ipc_first_return_ts(seL4_CPtr endpoint, seL4_UserContext *regs, seL4_Word *sender) {
    seL4_MessageInfo_t tag = seL4_Recv(endpoint, sender);
    return handle_fault_ipc_reentry_generic(tag, regs);
}

int l4bridge_fault_ipc_return_unknown_syscall_ts(seL4_CPtr endpoint, seL4_UserContext *regs, seL4_Word *sender) {
    seL4_Word *regs_raw = (seL4_Word *) regs;

    for(int i = 0; i < L4BRIDGE_NUM_REGISTERS; i++) {
        seL4_SetMR(i, regs_raw[i]);
    }

    seL4_MessageInfo_t tag = seL4_ReplyRecv(endpoint, seL4_MessageInfo_new(0, 0, 0, L4BRIDGE_NUM_REGISTERS), sender);
    return handle_fault_ipc_reentry_generic(tag, regs);
}

int l4bridge_fault_ipc_return_generic_ts(seL4_CPtr endpoint, seL4_UserContext *regs, seL4_Word *sender) {
    seL4_MessageInfo_t tag = seL4_ReplyRecv(endpoint, seL4_MessageInfo_new(0, 0, 0, 0), sender);
    return handle_fault_ipc_reentry_generic(tag, regs);
}

int l4bridge_resume(seL4_CPtr tcb) {
    return seL4_TCB_Resume(tcb);
}

int l4bridge_make_asid_pool_ts(seL4_CPtr untyped, seL4_CPtr out) {
    return seL4_X86_ASIDControl_MakePool(asid_control_cptr, untyped, CNODE_SLOT, out, seL4_WordBits);
}

int l4bridge_assign_asid_ts(seL4_CPtr pool, seL4_CPtr vspace) {
    return seL4_X86_ASIDPool_Assign(pool, vspace);
}

void l4bridge_delete_cap_ts(seL4_CPtr slot) {
    int error = seL4_CNode_Delete(CNODE_SLOT, slot, seL4_WordBits);
    if(error) {
        panic_str("[loader] l4bridge_delete_cap_ts: cannot delete cap\n");
    }
}

int l4bridge_mint_cap_ts(seL4_CPtr src, seL4_CPtr dst, seL4_Word badge) {
    return seL4_CNode_Mint(
        CNODE_SLOT, dst, seL4_WordBits,
        CNODE_SLOT, src, seL4_WordBits,
        seL4_AllRights,
        badge
    );
}

int l4bridge_badge_endpoint_to_user_thread_ts(seL4_CPtr src, seL4_CPtr dst_root, seL4_CPtr dst, seL4_Word dst_depth, seL4_Word badge) {
    return seL4_CNode_Mint(
        dst_root, dst, dst_depth,
        CNODE_SLOT, src, seL4_WordBits,
        seL4_CapRights_new(1, 0, 0, 1), // seL4_CanGrantReply | seL4_CanWrite
        badge
    );
}

int l4bridge_kipc_call(seL4_CPtr slot, seL4_Word data, seL4_Word *result) {
    seL4_SetMR(0, data);
    seL4_MessageInfo_t tag = seL4_Call(slot, seL4_MessageInfo_new(0, 0, 0, 1));
    if(seL4_MessageInfo_get_length(tag) != 1) {
        return 1;
    }
    *result = seL4_GetMR(0);
    return 0;
}

int l4bridge_kipc_recv(seL4_CPtr slot, seL4_Word *data, seL4_Word *sender_badge) {
    seL4_MessageInfo_t tag = seL4_Recv(slot, sender_badge);
    if(seL4_MessageInfo_get_length(tag) != 1) {
        return 1;
    }
    *data = seL4_GetMR(0);
    return 0;
}

// Thread safe.
void l4bridge_kipc_send_ts(seL4_CPtr slot, seL4_Word data) {
    seL4_SetMR(0, data);
    seL4_Send(slot, seL4_MessageInfo_new(0, 0, 0, 1));
}

// Thread safe.
void l4bridge_kipc_reply(seL4_Word result) {
    seL4_SetMR(0, result);
    seL4_Reply(seL4_MessageInfo_new(0, 0, 0, 1));
}

// Thread safe.
int l4bridge_kipc_reply_recv_ts(seL4_CPtr slot, seL4_Word result, seL4_Word *data, seL4_Word *sender_badge) {
    seL4_SetMR(0, result);
    seL4_MessageInfo_t tag = seL4_ReplyRecv(slot, seL4_MessageInfo_new(0, 0, 0, 1), sender_badge);

    if(seL4_MessageInfo_get_length(tag) != 1) {
        return 1;
    }
    *data = seL4_GetMR(0);
    return 0;
}

// Thread safe.
seL4_Word l4bridge_get_time_ts() {
    seL4_MessageInfo_t tag = seL4_Call(get_time_cptr, seL4_MessageInfo_new(0, 0, 0, 0));
    if(seL4_MessageInfo_get_length(tag) != 1) {
        panic_str("l4bridge_get_time_ts: bad response\n");
    }
    return seL4_GetMR(0);
}

// Thread safe.
int l4bridge_timer_set_period_ts(seL4_Word new_period) {
    seL4_SetMR(0, new_period);
    seL4_MessageInfo_t tag = seL4_Call(set_period_cptr, seL4_MessageInfo_new(0, 0, 0, 1));
    if(seL4_MessageInfo_get_length(tag) != 1) {
        panic_str("l4bridge_timer_set_period_ts: bad response\n");
    }
    return seL4_GetMR(0);
}

// Thread safe.
seL4_Word l4bridge_timer_wait_ts() {
    seL4_Word sender_badge = 0;
    seL4_MessageInfo_t tag = seL4_Recv(timer_event_cptr, &sender_badge);
    if(seL4_MessageInfo_get_length(tag) != 1) {
        panic_str("l4bridge_timer_wait_ts: bad response\n");
    }
    return seL4_GetMR(0);
}

// Thread safe.
int l4bridge_save_caller(seL4_CPtr dst) {
    return seL4_CNode_SaveCaller(CNODE_SLOT, dst, seL4_WordBits);
}

seL4_Word getcap(const char *name) {
    // XXX: Assuming sizeof(seL4_Word) == 8
    seL4_Word buf[4];
    if(sizeof(seL4_Word) != 8) {
        static_assert();
    }

    write_string_buf((char *) buf, name, 32);
    seL4_SetMR(0, buf[0]);
    seL4_SetMR(1, buf[1]);
    seL4_SetMR(2, buf[2]);
    seL4_SetMR(3, buf[3]);

    seL4_Call(GETCAP_CPTR, seL4_MessageInfo_new(0, 0, 0, 4));
    return seL4_GetMR(0);
}

void setup_twolevel_cspace() {
    if(alloc_cnode(TEMP_CPTR, TOPLEVEL_CNODE_BITS)) {
        panic_str("[loader] setup_twolevel_cspace: cannot allocate new root cnode\n");
    }

    if(seL4_CNode_Mutate(
        CNODE_SLOT, NEW_ROOT_CNODE_CPTR, seL4_WordBits,
        CNODE_SLOT, TEMP_CPTR, seL4_WordBits,
        seL4_CNode_CapData_new(0, seL4_WordBits - TOPLEVEL_CNODE_BITS - SECONDLEVEL_CNODE_BITS).words[0]
    )) {
        panic_str("[loader] setup_twolevel_cspace: cannot configure new cnode\n");
    }

    if(seL4_CNode_Mutate(
        NEW_ROOT_CNODE_CPTR, 0, seL4_WordBits - SECONDLEVEL_CNODE_BITS,
        CNODE_SLOT, CNODE_SLOT, seL4_WordBits,
        seL4_CNode_CapData_new(0, 0).words[0]
    )) {
        panic_str("[loader] setup_twolevel_cspace: cannot move old cnode\n");
    }

    if(seL4_TCB_SetSpace(TCB_SLOT, seL4_CapNull, NEW_ROOT_CNODE_CPTR, 0, PD_SLOT, 0)) {
        panic_str("[loader] setup_twolevel_cspace: cannot update cspace\n");
    }

    if(seL4_CNode_Move(
        NEW_ROOT_CNODE_CPTR, CNODE_SLOT, seL4_WordBits,
        NEW_ROOT_CNODE_CPTR, NEW_ROOT_CNODE_CPTR, seL4_WordBits
    )) {
        panic_str("[loader] setup_twolevel_cspace: cannot write back new cnode\n");
    }
}

int main();

void _start() {
    init_master_tls();
    seL4_SetIPCBuffer(ipc_buffer);

    putchar_cptr = getcap("putchar");
    alloc_untyped_cptr = getcap("alloc_untyped");
    alloc_cnode_cptr = getcap("alloc_cnode");
    timer_event_cptr = getcap("timer_event");
    set_period_cptr = getcap("set_period");
    get_time_cptr = getcap("get_time");
    asid_control_cptr = getcap("asid_control");

    main();
}

int main() {
    print_str("ZcLoader started.\n");
    setup_twolevel_cspace();
    print_str("CSpace reconfigured, entering Rust.\n");
    rust_start();
    print_str("rust_start unexpectedly returned\n");
    while(1) {}
}
/*
int bcmp(const void *_s1, const void *_s2, unsigned long n) {
    const char *s1 = _s1;
    const char *s2 = _s2;
    for(int i = 0; i < n; i++) if(s1[i] != s2[i]) return 1;
    return 0;
}

void * memset(void *ptr, int value, unsigned long num) {
    char *buf = ptr;
    for(int i = 0; i < num; i++) {
        buf[i] = value;
    }
    return ptr;
}*/

void __assert_fail(const char * assertion, const char * file, int line, const char * function) {
    while(1) {}
}
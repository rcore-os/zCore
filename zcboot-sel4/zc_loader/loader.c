#include <sel4/sel4.h>

#define CNODE_SLOT 1
#define FAULT_ENDPOINT_SLOT 2
#define PD_SLOT 3
#define ASID_POOL_SLOT 4
#define TCB_SLOT 5

#define GETCAP_CPTR 8

#define TEMP_CPTR 63
#define NEW_ROOT_CNODE_CPTR 62

#define ZCDAEMON_IPCBUF_VADDR 0x3000000
#define TOPLEVEL_CNODE_BITS 12
#define SECONDLEVEL_CNODE_BITS 12

#ifndef MASK
#define MASK(n) (BIT(n)-1ul)
#endif

#ifndef BIT
#define BIT(n) (1ul<<(n))
#endif 

// 1M stack for Rust
#define MAIN_STACK_SIZE 1048576
static char MAIN_STACK[MAIN_STACK_SIZE];

// TLS
// FIXME: Actually load TLS.
#define TLS_SIZE 65536
static char TLS[TLS_SIZE];

// IPC buffer
static seL4_IPCBuffer *ipc_buffer = (seL4_IPCBuffer *) ZCDAEMON_IPCBUF_VADDR;

extern void rust_start();

// Fails if not stripped before linking.
void static_assert();

seL4_CPtr putchar_cptr = 0;
seL4_CPtr alloc_frame_cptr = 0;
seL4_CPtr alloc_cnode_cptr = 0;

unsigned char toplevel_cnode_slot_allocated[BIT(TOPLEVEL_CNODE_BITS)];

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

void init_master_tls() {
    // reference: https://wiki.osdev.org/Thread_Local_Storage
    seL4_Word thread_area = (seL4_Word) TLS + TLS_SIZE - 0x1000;
    * (seL4_Word *) thread_area = thread_area;
    seL4_SetTLSBase(thread_area);
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

int l4bridge_alloc_frame(seL4_CPtr slot, seL4_Word *paddr_out) {
    seL4_SetCapReceivePath(CNODE_SLOT, slot, seL4_WordBits);
    seL4_MessageInfo_t tag = seL4_Call(alloc_frame_cptr, seL4_MessageInfo_new(0, 0, 0, 0));
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

int l4bridge_ensure_cslot(seL4_CPtr slot) {
    int index = (slot >> SECONDLEVEL_CNODE_BITS) & MASK(TOPLEVEL_CNODE_BITS);
    if(toplevel_cnode_slot_allocated[index]) return 0;

    if(alloc_cnode(TEMP_CPTR, 12)) {
        return 1;
    }

    // XXX: Seems that the slot index is truncated from the MSB.
    if(seL4_CNode_Mutate(
        CNODE_SLOT, index, seL4_WordBits - SECONDLEVEL_CNODE_BITS,
        CNODE_SLOT, TEMP_CPTR, seL4_WordBits,
        seL4_CNode_CapData_new(0, 0).words[0]
    )) {
        return 1;
    }

    toplevel_cnode_slot_allocated[index] = 1;
    return 0;
}

void l4bridge_delete_cap(seL4_CPtr slot) {
    int error = seL4_CNode_Delete(CNODE_SLOT, slot, seL4_WordBits);
    if(error) {
        print_str("[loader] l4bridge_delete_cap: cannot delete cap\n");
    }
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

    // Identity-mapped first slot
    toplevel_cnode_slot_allocated[0] = 1;
}

void _start() {
    init_master_tls();
    seL4_SetIPCBuffer(ipc_buffer);

    putchar_cptr = getcap("putchar");
    alloc_frame_cptr = getcap("alloc_frame");
    alloc_cnode_cptr = getcap("alloc_cnode");

    unsigned long stack_top = (unsigned long) MAIN_STACK + MAIN_STACK_SIZE;
    asm volatile (
        "movq %0, %%rsp\n"
        "call main\n"
        "ud2"
        :: "r" (stack_top)
    );
}

int main() {
    print_str("Starting zCore.\n");
    setup_twolevel_cspace();
    print_str("CSpace reconfigured.\n");
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
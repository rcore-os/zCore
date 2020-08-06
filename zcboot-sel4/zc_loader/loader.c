#include <sel4/sel4.h>

#define CNODE_SLOT 1
#define FAULT_ENDPOINT_SLOT 2
#define PD_SLOT 3
#define ASID_POOL_SLOT 4
#define TCB_SLOT 5

#define GETCAP_CPTR 8

// 1M stack for Rust
#define MAIN_STACK_SIZE 1048576
static char MAIN_STACK[MAIN_STACK_SIZE];

// IPC buffer
static seL4_IPCBuffer ipc_buffer = {0};

void rust_start();
void stbss();
void etbss();
void ethread();

// Fails if not stripped before linking.
void static_assert();

seL4_CPtr putchar_cptr = 0;

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

void l4bridge_putchar(char c) {
    seL4_SetMR(0, c);
    seL4_MessageInfo_t tag = seL4_MessageInfo_new(0, 0, 0, 1);
    seL4_Call(putchar_cptr, tag);
}

void l4bridge_yield() {
    seL4_Yield();
}

void init_master_tls() {
    // reference: https://wiki.osdev.org/Thread_Local_Storage
    * (seL4_Word *) etbss = (seL4_Word) etbss;
    seL4_SetTLSBase((seL4_Word) etbss);
}

void write_string_buf(char *dst, const char *src, int dst_size) {
    for(int i = 0; i < dst_size; i++) {
        dst[i] = src[i];
        if(src[i] == 0) return;
    }
    dst[dst_size - 1] = 0;
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

void print_str(const char *s) {
    while(*s) {
        l4bridge_putchar(*s);
        s++;
    }
}

void _start() {
    init_master_tls();
    seL4_SetIPCBuffer(&ipc_buffer);

    putchar_cptr = getcap("putchar");
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
    rust_start();
    print_str("rust_start unexpectedly returned\n");
    while(1) {}
}

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
}

void __assert_fail(const char * assertion, const char * file, int line, const char * function) {
    while(1) {}
}
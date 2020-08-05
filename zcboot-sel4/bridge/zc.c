#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include <sel4/sel4.h>

static seL4_Word getcap_cptr;
static seL4_Word putchar_cptr;

// 1M stack for Rust
#define RUST_STACK_SIZE 1048576

static char RUST_STACK[RUST_STACK_SIZE];

void run_rust_start() {
    unsigned long stack_top = (unsigned long) RUST_STACK + RUST_STACK_SIZE;

    asm volatile (
        "movq %0, %%rsp\n"
        "call rust_start\n"
        "ud2"
        :: "r" (stack_top)
    );
}

seL4_Word getcap(const char *name) {
    seL4_Word buf[4];
    assert(sizeof(seL4_Word) == 8);

    strncpy((char *) buf, name, 32);
    seL4_SetMR(0, buf[0]);
    seL4_SetMR(1, buf[1]);
    seL4_SetMR(2, buf[2]);
    seL4_SetMR(3, buf[3]);

    seL4_Call(getcap_cptr, seL4_MessageInfo_new(0, 0, 0, 4));
    return seL4_GetMR(0);
}

int main(int argc, char *argv[]) {
    if(argc != 2) {
        printf("Bad argc\n");
        return 1;
    }

    getcap_cptr = atoi(argv[1]);
    putchar_cptr = getcap("putchar");

    run_rust_start();
    printf("Unexpected return from rust_start\n");
    return 1;
}

void l4bridge_putchar(char c) {
    seL4_SetMR(0, c);
    seL4_MessageInfo_t tag = seL4_MessageInfo_new(0, 0, 0, 1);
    seL4_Call(putchar_cptr, tag);
}

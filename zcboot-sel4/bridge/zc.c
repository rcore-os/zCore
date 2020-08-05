#include <stdio.h>
#include <stdlib.h>
#include <sel4/sel4.h>

static seL4_Word putchar_cptr;

void rust_start();

int main(int argc, char *argv[]) {
    printf("Starting zCore.\n");

    if(argc != 2) {
        printf("Bad argc\n");
        return 1;
    }

    putchar_cptr = atoi(argv[1]);

    rust_start();
    printf("Unexpected return from rust_start\n");
    return 1;
}

void l4bridge_putchar(char c) {
    seL4_SetMR(0, c);
    seL4_MessageInfo_t tag = seL4_MessageInfo_new(0, 0, 0, 1);
    seL4_Call(putchar_cptr, tag);
}

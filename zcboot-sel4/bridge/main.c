#include <stdio.h>

#include <sel4platsupport/platsupport.h>
#include <sel4platsupport/bootinfo.h>
#include <allocman/allocman.h>
#include <allocman/bootstrap.h>
#include <allocman/vka.h>
#include <sel4utils/vspace.h>
#include <sel4utils/process.h>
#include <simple/simple.h>
#include <simple-default/simple-default.h>

seL4_BootInfo *boot_info;
simple_t simple;
allocman_t *allocman;
vspace_t vspace;
vka_t vka;
static sel4utils_alloc_data_t vspace_data;

#define ALLOCATOR_STATIC_POOL_SIZE BIT(23)
static char allocator_mem_pool[ALLOCATOR_STATIC_POOL_SIZE];

#define ALLOCATOR_VIRTUAL_POOL_SIZE BIT(28)

#define ZCDAEMON_BADGE_PUTCHAR 0x10

void load_zc();
seL4_Word setup_ipc(sel4utils_process_t *process, const vka_object_t *ep_object, seL4_Word badge);

int main(int argc, char *argv[]) {
    int error;

    boot_info = platsupport_get_bootinfo();
    ZF_LOGF_IF(boot_info == NULL, "cannot get boot info");

    simple_default_init_bootinfo(&simple, boot_info);

    allocman = bootstrap_use_current_simple(&simple, ALLOCATOR_STATIC_POOL_SIZE,
                                            allocator_mem_pool);
    ZF_LOGF_IF(allocman == NULL, "cannot initialize allocman");

    allocman_make_vka(&vka, allocman);
    error = sel4utils_bootstrap_vspace_with_bootinfo_leaky(
        &vspace,
        &vspace_data,
        simple_get_pd(&simple),
        &vka,
        boot_info
    );
    ZF_LOGF_IFERR(error, "cannot bootstrap vspace");

    error = platsupport_serial_setup_simple(&vspace, &simple, &vka);
    ZF_LOGF_IFERR(error, "cannot setup serial");

    simple_print(&simple);

    printf("ZcBoot bridge started.\n");

    load_zc();

    return 0;
}

void load_zc() {
    int error;
    void *vaddr;
    reservation_t virtual_reservation;

    virtual_reservation = vspace_reserve_range(
        &vspace,
        ALLOCATOR_VIRTUAL_POOL_SIZE,
        seL4_AllRights,
        1,
        &vaddr
    );
    assert(virtual_reservation.res);
    bootstrap_configure_virtual_pool(
        allocman, vaddr,
        ALLOCATOR_VIRTUAL_POOL_SIZE, simple_get_pd(&simple)
    );

    // Create process.
    sel4utils_process_t new_process;
    sel4utils_process_config_t config = process_config_default_simple(&simple, "zc", seL4_MaxPrio);
    error = sel4utils_configure_process_custom(&new_process, &vka, &vspace, config);
    ZF_LOGF_IFERR(error, "failed to configure process");

    // Setup IPC.
    vka_object_t ep_object= {0};
    error = vka_alloc_endpoint(&vka, &ep_object);
    ZF_LOGF_IFERR(error, "Failed to allocate ep_object.\n");

    // 0. putchar
    seL4_Word child_putchar_cptr = setup_ipc(&new_process, &ep_object, ZCDAEMON_BADGE_PUTCHAR);

    const int ARG_MAX_LEN_WITHOUT_TERM = 127;

    // Prepare arguments.
    char *arglist[2];
    arglist[0] = "zc";
    arglist[1] = malloc(ARG_MAX_LEN_WITHOUT_TERM + 1);
    snprintf(arglist[1], ARG_MAX_LEN_WITHOUT_TERM, "%lu", child_putchar_cptr);

    // Spawn process.
    error = sel4utils_spawn_process_v(&new_process, &vka, &vspace, 2, arglist, 1);
    ZF_LOGF_IFERR(error, "failed to spawn process");

    // Handle IPC.
    while(1) {
        seL4_Word sender_badge;
        seL4_MessageInfo_t tag = seL4_Recv(ep_object.cptr, &sender_badge);
        switch(sender_badge) {
            case ZCDAEMON_BADGE_PUTCHAR: {
                char ch = (char) seL4_GetMR(0);
                putchar(ch);
                fflush(stdout);
                seL4_Reply(tag);
                break;
            }
            default: {
                printf("Unknown sender badge: %lx", sender_badge);
                break;
            }
        }
    }
}

seL4_Word setup_ipc(sel4utils_process_t *process, const vka_object_t *ep_object, seL4_Word badge) {
    cspacepath_t ep_cap_path;
    vka_cspace_make_path(&vka, ep_object->cptr, &ep_cap_path);

    seL4_CPtr new_ep_cap = 0;
    new_ep_cap = sel4utils_mint_cap_to_process(process, ep_cap_path, seL4_AllRights, badge);
    ZF_LOGF_IF(new_ep_cap == 0, "Failed to mint cap to new process.");

    return new_ep_cap;
}
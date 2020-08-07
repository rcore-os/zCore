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

#define ZCDAEMON_IPCBUF_VADDR 0x3000000
#define FAULT_HANDLER_IPCBUF_VADDR 0x2000000

seL4_BootInfo *boot_info;
simple_t simple;
allocman_t *allocman;
vspace_t vspace;
vka_t vka;
static sel4utils_alloc_data_t vspace_data;

#define ALLOCATOR_STATIC_POOL_SIZE BIT(23)
static char allocator_mem_pool[ALLOCATOR_STATIC_POOL_SIZE];

#define ALLOCATOR_VIRTUAL_POOL_SIZE BIT(28)

#define ZCDAEMON_BADGE_GETCAP 0x10
#define ZCDAEMON_BADGE_PUTCHAR 0x11
#define ZCDAEMON_BADGE_ALLOC_FRAME 0x12

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

int map_remote_frame(sel4utils_process_t *process, seL4_Word vaddr, vka_object_t *obj_out) {
    int error;

    error = vka_alloc_frame(&vka, seL4_PageBits, obj_out); // 4K frame
    if(error) return error;

    error = seL4_ARCH_Page_Map(
        obj_out->cptr,
        process->pd.cptr,
        vaddr,
        seL4_AllRights,
        seL4_ARCH_Default_VMAttributes
    );
    if(error) {
        vka_object_t new_page_table;

        error = vka_alloc_page_table(&vka, &new_page_table);
        if(error) return error;

        error = seL4_ARCH_PageTable_Map(new_page_table.cptr, process->pd.cptr, vaddr, seL4_ARCH_Default_VMAttributes);
        if(error) return error;

        error = seL4_ARCH_Page_Map(
            obj_out->cptr,
            process->pd.cptr,
            vaddr,
            seL4_AllRights,
            seL4_ARCH_Default_VMAttributes
        );
        if(error) return error;
    }

    return 0;
}

int prepare_ipc_buffer(sel4utils_process_t *process) {
    int error;
    vka_object_t obj;

    error = map_remote_frame(process, ZCDAEMON_IPCBUF_VADDR, &obj);
    if(error) return error;

    error = seL4_TCB_SetIPCBuffer(process->thread.tcb.cptr, ZCDAEMON_IPCBUF_VADDR, obj.cptr);
    if(error) return error;

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

    // Setup IPC.
    vka_object_t ep_object= {0};
    error = vka_alloc_endpoint(&vka, &ep_object);
    ZF_LOGF_IFERR(error, "Failed to allocate ep_object.\n");

    // Create process.
    sel4utils_process_t new_process;
    sel4utils_process_config_t config = process_config_default_simple(&simple, "zcboot-sel4", seL4_MaxPrio);
    config = process_config_create_cnode(config, 12); // 4K entries
    error = sel4utils_configure_process_custom(&new_process, &vka, &vspace, config);
    ZF_LOGF_IFERR(error, "failed to configure process");

    // Prepare IPC frame.
    error = prepare_ipc_buffer(&new_process);
    ZF_LOGF_IFERR(error, "failed to prepare ipc buffer");

    // Caps.
    seL4_Word child_getcap_cptr = setup_ipc(&new_process, &ep_object, ZCDAEMON_BADGE_GETCAP);
    seL4_Word child_putchar_cptr = setup_ipc(&new_process, &ep_object, ZCDAEMON_BADGE_PUTCHAR);
    seL4_Word child_alloc_frame_cptr = setup_ipc(&new_process, &ep_object, ZCDAEMON_BADGE_ALLOC_FRAME);

    const int ARG_MAX_LEN_WITHOUT_TERM = 31;

    // Prepare arguments.
    char *arglist[2];
    arglist[0] = "zc";
    arglist[1] = malloc(ARG_MAX_LEN_WITHOUT_TERM + 1);
    snprintf(arglist[1], ARG_MAX_LEN_WITHOUT_TERM, "%lu", child_getcap_cptr);

    // Spawn process.
    error = sel4utils_spawn_process_v(&new_process, &vka, &vspace, 2, arglist, 1);
    ZF_LOGF_IFERR(error, "failed to spawn process");

    printf("Spawned process.\n");

    // Handle IPC.
    while(1) {
        seL4_Word sender_badge;
        seL4_MessageInfo_t tag = seL4_Recv(ep_object.cptr, &sender_badge);
        switch(sender_badge) {
            case ZCDAEMON_BADGE_GETCAP: {
                if(seL4_MessageInfo_get_length(tag) != 4) {
                    ZF_LOGI("ZCDAEMON_BADGE_GETCAP: Bad tag length");
                    break;
                }
                assert(sizeof(seL4_Word) == 8); // supports 64-bit platforms only

                // Collect name.
                seL4_Word w0 = seL4_GetMR(0), w1 = seL4_GetMR(1), w2 = seL4_GetMR(2), w3 = seL4_GetMR(3);
                char name[32];
                memcpy(name + 0, &w0, 8);
                memcpy(name + 8, &w1, 8);
                memcpy(name + 16, &w2, 8);
                memcpy(name + 24, &w3, 8);
                name[31] = 0;

                if(strcmp(name, "putchar") == 0) {
                    seL4_SetMR(0, child_putchar_cptr);
                } else if(strcmp(name, "alloc_frame") == 0) {
                    seL4_SetMR(0, child_alloc_frame_cptr);
                } else {
                    printf("Unknown cap name: %s\n", name);
                    seL4_SetMR(0, 0);
                }
                seL4_Reply(seL4_MessageInfo_new(0, 0, 0, 1));
                break;
            }
            case ZCDAEMON_BADGE_PUTCHAR: {
                if(seL4_MessageInfo_get_length(tag) != 1) {
                    ZF_LOGI("ZCDAEMON_BADGE_PUTCHAR: Bad tag length");
                    break;
                }
                char ch = (char) seL4_GetMR(0);
                putchar(ch);
                fflush(stdout);
                seL4_Reply(seL4_MessageInfo_new(0, 0, 0, 0));
                break;
            }
            case ZCDAEMON_BADGE_ALLOC_FRAME: {
                if(seL4_MessageInfo_get_length(tag) != 0) {
                    ZF_LOGI("ZCDAEMON_BADGE_ALLOC_FRAME: Bad tag length");
                    break;
                }
                vka_object_t frame;
                error = vka_alloc_frame(&vka, seL4_PageBits, &frame);
                if(error) {
                    seL4_Reply(seL4_MessageInfo_new(1, 0, 0, 0));
                    break;
                }
                seL4_SetCap(0, frame.cptr);
                seL4_SetMR(0, vka_object_paddr(&vka, &frame));
                seL4_Reply(seL4_MessageInfo_new(0, 0, 1, 1));

                // We cannot free the underlying memory for the object.
                // Instead, free the CPtr only.
                cspacepath_t path;
                vka_cspace_make_path(&vka, frame.cptr, &path);
                seL4_CNode_Delete(path.root, path.capPtr, path.capDepth);
                vka_cspace_free(&vka, frame.cptr);

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

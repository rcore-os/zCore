#include <stdio.h>

#include <sel4platsupport/platsupport.h>
#include <sel4platsupport/bootinfo.h>
#include <sel4platsupport/io.h>
#include <allocman/allocman.h>
#include <allocman/bootstrap.h>
#include <allocman/vka.h>
#include <sel4utils/vspace.h>
#include <sel4utils/process.h>
#include <simple/simple.h>
#include <simple-default/simple-default.h>
#include <platsupport/plat/timer.h>
#include <platsupport/ltimer.h>

#define ZCDAEMON_IPCBUF_VADDR 0x3000000

seL4_BootInfo *boot_info;
simple_t simple;
allocman_t *allocman;
vspace_t vspace;
vka_t vka;
static sel4utils_alloc_data_t vspace_data;
ltimer_t timer;
ps_io_ops_t timer_ops = {{0}};

#define ALLOCATOR_STATIC_POOL_SIZE BIT(21)
static char allocator_mem_pool[ALLOCATOR_STATIC_POOL_SIZE];

#define ALLOCATOR_VIRTUAL_POOL_SIZE BIT(28)

#define ZCDAEMON_BADGE_GETCAP 0xff10
#define ZCDAEMON_BADGE_PUTCHAR 0xff11
#define ZCDAEMON_BADGE_ALLOC_UNTYPED 0xff12
#define ZCDAEMON_BADGE_ALLOC_CNODE 0xff13
#define ZCDAEMON_BADGE_TIMER_SET_PERIOD 0xff14
#define ZCDAEMON_BADGE_GET_TIME 0xff15

void load_zc();
static seL4_Word setup_ipc_with_cptr(sel4utils_process_t *process, seL4_CPtr cptr, seL4_Word badge);
static seL4_Word setup_ipc(sel4utils_process_t *process, const vka_object_t *ep_object, seL4_Word badge);
static void setup_timer();

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

    //simple_print(&simple);

    printf("ZcBoot started.\n");

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

void free_cptr_for_object(vka_object_t *obj) {
    cspacepath_t path;
    vka_cspace_make_path(&vka, obj->cptr, &path);
    int error = seL4_CNode_Delete(path.root, path.capPtr, path.capDepth);
    if(error) {
        printf("failed to delete cnode\n");
    }
    vka_cspace_free(&vka, obj->cptr);
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

    // Setup timer.
    setup_timer();

    // Setup IPC.
    vka_object_t ep_object= {0};
    error = vka_alloc_endpoint(&vka, &ep_object);
    ZF_LOGF_IFERR(error, "Failed to allocate ep_object.\n");

    // Create process.
    sel4utils_process_t new_process;
    sel4utils_process_config_t config = process_config_default_simple(&simple, "zcboot-sel4", seL4_MaxPrio);
    config = process_config_create_cnode(config, 12); // 4K entries
    config = process_config_mcp(config, seL4_MaxPrio);
    error = sel4utils_configure_process_custom(&new_process, &vka, &vspace, config);
    ZF_LOGF_IFERR(error, "failed to configure process");

    // Prepare IPC frame.
    error = prepare_ipc_buffer(&new_process);
    ZF_LOGF_IFERR(error, "failed to prepare ipc buffer");

    // Periodic timer events.
    vka_object_t timer_event_channel;
    error = vka_alloc_endpoint(&vka, &timer_event_channel);

    // Caps.
    seL4_Word child_getcap_cptr = setup_ipc(&new_process, &ep_object, ZCDAEMON_BADGE_GETCAP);
    seL4_Word child_putchar_cptr = setup_ipc(&new_process, &ep_object, ZCDAEMON_BADGE_PUTCHAR);
    seL4_Word child_alloc_untyped_cptr = setup_ipc(&new_process, &ep_object, ZCDAEMON_BADGE_ALLOC_UNTYPED);
    seL4_Word child_alloc_cnode_cptr = setup_ipc(&new_process, &ep_object, ZCDAEMON_BADGE_ALLOC_CNODE);
    seL4_Word child_set_period_cptr = setup_ipc(&new_process, &ep_object, ZCDAEMON_BADGE_TIMER_SET_PERIOD);
    seL4_Word child_get_time_cptr = setup_ipc(&new_process, &ep_object, ZCDAEMON_BADGE_GET_TIME);
    seL4_Word child_timer_event_cptr = setup_ipc(&new_process, &timer_event_channel, 0);
    seL4_Word child_asid_control_cptr = setup_ipc_with_cptr(&new_process, seL4_CapASIDControl, 0);

    ZF_LOGF_IFERR(error, "Failed to allocate timer event channel.\n");

    const int ARG_MAX_LEN_WITHOUT_TERM = 31;

    // Prepare arguments.
    char *arglist[2];
    arglist[0] = "zc";
    arglist[1] = malloc(ARG_MAX_LEN_WITHOUT_TERM + 1);
    snprintf(arglist[1], ARG_MAX_LEN_WITHOUT_TERM, "%lu", child_getcap_cptr);

    // Spawn process.
    error = sel4utils_spawn_process_v(&new_process, &vka, &vspace, 2, arglist, 1);
    ZF_LOGF_IFERR(error, "failed to spawn process");

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
                } else if(strcmp(name, "alloc_untyped") == 0) {
                    seL4_SetMR(0, child_alloc_untyped_cptr);
                } else if(strcmp(name, "alloc_cnode") == 0) {
                    seL4_SetMR(0, child_alloc_cnode_cptr);
                } else if(strcmp(name, "timer_event") == 0) {
                    seL4_SetMR(0, child_timer_event_cptr);
                } else if(strcmp(name, "set_period") == 0) {
                    seL4_SetMR(0, child_set_period_cptr);
                } else if(strcmp(name, "get_time") == 0) {
                    seL4_SetMR(0, child_get_time_cptr);
                } else if(strcmp(name, "asid_control") == 0) {
                    seL4_SetMR(0, child_asid_control_cptr);
                } else {
                    ZF_LOGF("Unknown cap name: %s", name);
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
            case ZCDAEMON_BADGE_ALLOC_UNTYPED: {
                if(seL4_MessageInfo_get_length(tag) != 1) {
                    ZF_LOGE("ZCDAEMON_BADGE_ALLOC_UNTYPED: Bad tag length");
                    seL4_Reply(seL4_MessageInfo_new(1, 0, 0, 0));
                    break;
                }

                uint32_t bits = seL4_GetMR(0);

                seL4_CPtr frame_cptr;
                cspacepath_t frame_path;

                error = vka_cspace_alloc(&vka, &frame_cptr);
                if(error) {
                    ZF_LOGE("ZCDAEMON_BADGE_ALLOC_UNTYPED: vka_cspace_alloc");
                    seL4_Reply(seL4_MessageInfo_new(1, 0, 0, 0));
                    break;
                }

                vka_cspace_make_path(&vka, frame_cptr, &frame_path);
                seL4_Word cookie = allocman_utspace_alloc(allocman, bits, seL4_UntypedObject, &frame_path, 0, &error);
                if(error) {
                    //ZF_LOGE("ZCDAEMON_BADGE_ALLOC_UNTYPED: allocman_utspace_alloc");
                    seL4_Reply(seL4_MessageInfo_new(1, 0, 0, 0));
                    break;
                }

                seL4_SetCap(0, frame_cptr);
                seL4_SetMR(0, allocman_utspace_paddr(allocman, cookie, bits));
                seL4_Reply(seL4_MessageInfo_new(0, 0, 1, 1));

                seL4_CNode_Delete(frame_path.root, frame_path.capPtr, frame_path.capDepth);
                vka_cspace_free(&vka, frame_cptr);
                break;
            }
            case ZCDAEMON_BADGE_ALLOC_CNODE: {
                if(seL4_MessageInfo_get_length(tag) != 1) {
                    ZF_LOGI("ZCDAEMON_BADGE_ALLOC_CNODE: Bad tag length");
                    break;
                }
                uint32_t size_bits = seL4_GetMR(0);
                vka_object_t cnode;
                error = vka_alloc_cnode_object(&vka, size_bits, &cnode);
                if(error) {
                    seL4_Reply(seL4_MessageInfo_new(1, 0, 0, 0));
                    break;
                }
                seL4_SetCap(0, cnode.cptr);
                seL4_Reply(seL4_MessageInfo_new(0, 0, 1, 0));
                free_cptr_for_object(&cnode);
                break;
            }
            case ZCDAEMON_BADGE_TIMER_SET_PERIOD: {
                if(seL4_MessageInfo_get_length(tag) != 1) {
                    ZF_LOGF("ZCDAEMON_BADGE_TIMER_SET_PERIOD: Bad tag length");
                }
                uint64_t new_period = seL4_GetMR(0);
                error = ltimer_set_timeout(&timer, new_period, TIMEOUT_PERIODIC);
                seL4_SetMR(0, error);
                seL4_Reply(seL4_MessageInfo_new(0, 0, 0, 1));
                break;
            }
            case ZCDAEMON_BADGE_GET_TIME: {
                uint64_t time = 0;
                ltimer_get_time(&timer, &time);
                seL4_SetMR(0, time);
                seL4_Reply(seL4_MessageInfo_new(0, 0, 0, 1));
                break;
            }
            case 1: {
                // FIXME: Custom badge for timer?
                sel4platsupport_irq_handle(&timer_ops.irq_ops, MINI_IRQ_INTERFACE_NTFN_ID, 1);

                uint64_t time = 0;
                ltimer_get_time(&timer, &time);

                seL4_SetMR(0, time);
                seL4_NBSend(timer_event_channel.cptr, seL4_MessageInfo_new(0, 0, 0, 1));
                break;
            }
            default: {
                ZF_LOGF("Unknown sender badge: %lx", sender_badge);
                break;
            }
        }
    }
}

static seL4_Word setup_ipc_with_cptr(sel4utils_process_t *process, seL4_CPtr cptr, seL4_Word badge) {
    cspacepath_t ep_cap_path;
    vka_cspace_make_path(&vka, cptr, &ep_cap_path);

    seL4_CPtr new_ep_cap = 0;
    new_ep_cap = sel4utils_mint_cap_to_process(process, ep_cap_path, seL4_AllRights, badge);
    ZF_LOGF_IF(new_ep_cap == 0, "Failed to mint cap to new process.");

    return new_ep_cap;
}

static seL4_Word setup_ipc(sel4utils_process_t *process, const vka_object_t *ep_object, seL4_Word badge) {
    return setup_ipc_with_cptr(process, ep_object->cptr, badge);
}

static void setup_timer() {
    int error;
    vka_object_t ntfn_object = {0};
    error = vka_alloc_notification(&vka, &ntfn_object);
    ZF_LOGF_IFERR(error, "cannot alloc notification");

    error = sel4platsupport_new_malloc_ops(&timer_ops.malloc_ops);
    assert(error == 0);
    error = sel4platsupport_new_io_mapper(&vspace, &vka, &timer_ops.io_mapper);
    assert(error == 0);
    error = sel4platsupport_new_fdt_ops(&timer_ops.io_fdt, &simple, &timer_ops.malloc_ops);
    assert(error == 0);
    error = sel4platsupport_new_mini_irq_ops(&timer_ops.irq_ops, &vka, &simple, &timer_ops.malloc_ops,
                                                ntfn_object.cptr, MASK(seL4_BadgeBits));
    assert(error == 0);
    error = sel4platsupport_new_arch_ops(&timer_ops, &simple, &vka);
    assert(error == 0);

    error = ltimer_default_init(&timer, timer_ops, NULL, NULL);
    assert(error == 0);

    error = seL4_TCB_BindNotification(simple_get_tcb(&simple), ntfn_object.cptr);
    assert(error == 0);
}

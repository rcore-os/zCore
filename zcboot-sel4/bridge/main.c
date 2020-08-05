#include <stdio.h>

#include <sel4platsupport/platsupport.h>
#include <sel4platsupport/bootinfo.h>
#include <allocman/allocman.h>
#include <allocman/bootstrap.h>
#include <allocman/vka.h>
#include <sel4utils/vspace.h>
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

int main(int argc, char *argv[]) {
    int error;

    // XXX: Why?
    platsupport_serial_setup_bootinfo_failsafe();

    boot_info = platsupport_get_bootinfo();
    ZF_LOGF_IF(boot_info == NULL, "cannot get boot info");

    simple_default_init_bootinfo(&simple, boot_info);
    simple_print(&simple);

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


    printf("Hello world from zcboot bridge\n");
    return 0;
}
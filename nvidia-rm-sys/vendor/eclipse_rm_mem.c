/*
 * OUR code, not NVIDIA's -- Eclipse's implementation of the four
 * os-interface.h system-memory entry points the real RM calls for every
 * ADDR_SYSMEM memory descriptor:
 *
 *     osAllocPagesInternal / osFreePagesInternal
 *     osMapSystemMemory    / osUnmapSystemMemory
 *
 * NVIDIA's own versions live in arch/nvalloc/unix/src/os.c -- the Linux
 * platform layer Eclipse does not vendor -- and are built on nv_alloc_pages
 * / nv_alloc_kernel_mapping / nv_state_t. Eclipse instead backs them with
 * its own contiguous DMA frame allocator and kernel physmap, already
 * exported as C symbols by kernel-hal (drivers.rs, #[no_mangle]) and used
 * the same way by the AHCI/NVMe/e1000e drivers. Those symbols are resolved
 * at the final zcore link, exactly like glue.c's nvrm_shim_log_raw.
 *
 * Contract (transcribed from the real os.c + mem_desc.c):
 *   - _memdescAllocInternal (mem_desc.c) calls osAllocPages(pMemDesc) for
 *     ADDR_SYSMEM. It must fill the memdesc's CPU PTE array with physical
 *     page addresses and stash an opaque per-alloc handle via
 *     memdescSetMemData so the free/map paths can find the block again.
 *   - memdescMap -> osMapSystemMemory turns that handle into a CPU kernel
 *     virtual address (GspMsgQueuesInit fails with NV_ERR_NO_MEMORY when
 *     this returns NULL -- message_queue_cpu.c:288).
 *
 * Eclipse allocates the whole descriptor as one physically contiguous DMA
 * block (valid for both contiguous and non-contiguous memdescs -- the PTE
 * array is just filled with base + i*RM_PAGE_SIZE), and every physical page
 * of RAM is already mapped in the kernel physmap, so a kernel mapping is
 * just drivers_phys_to_virt(base) with no per-mapping page-table work.
 */
#include "os/os.h"
#include "nvos.h"
#include "gpu/mem_mgr/mem_desc.h"
#include "gpu/mem_mgr/rm_page_size.h"
#include "nvport/nvport.h"

/* Eclipse kernel-hal seams (kernel-hal/src/drivers.rs, all #[no_mangle]).
 * usize/PhysAddr/VirtAddr are all 64-bit on x86_64, matching NvU64. */
extern NvU64 drivers_dma_alloc(NvU64 pages);
extern NvS32 drivers_dma_dealloc(NvU64 paddr, NvU64 pages);
extern NvU64 drivers_phys_to_virt(NvU64 paddr);
extern NvS32 drivers_dma_mark_uncached(NvU64 paddr, NvU64 pages);

/*
 * memData handle: remembers the contiguous block behind a memdesc so
 * osFreePagesInternal can return it to the frame allocator and
 * osMapSystemMemory can recover its base directly (rather than re-deriving
 * it from a possibly IOMMU-translated PTE array).
 */
typedef struct
{
    NvU64 physBase;   /* physical base of the contiguous block           */
    NvU64 pageCount;  /* number of RM_PAGE_SIZE (4 KiB) pages allocated  */
} EclipseSysmemAlloc;

NV_STATUS osAllocPagesInternal(MEMORY_DESCRIPTOR *pMemDesc)
{
    NvU64               size       = memdescGetSize(pMemDesc);
    NvU64               pageCount  = (size + RM_PAGE_SIZE - 1) / RM_PAGE_SIZE;
    NvU64               physBase;
    NvU32               pteCount;
    NvU32               i;
    RmPhysAddr         *pteArray;
    EclipseSysmemAlloc *pAlloc;
    NV_STATUS           status;

    memdescSetMemData(pMemDesc, NULL, NULL);

    if (pageCount == 0)
        return NV_ERR_INVALID_ARGUMENT;

    physBase = drivers_dma_alloc(pageCount);
    if (physBase == 0)
        return NV_ERR_NO_MEMORY;

    /* Honor an uncached request the same way the Linux path's GFP flags do,
     * by remapping the physmap image of these pages UC (drivers.rs). The
     * GSP RPC queues are NV_MEMORY_CACHED and skip this; the sysmem flush
     * buffer is NV_MEMORY_UNCACHED and needs it. */
    if (memdescGetCpuCacheAttrib(pMemDesc) == NV_MEMORY_UNCACHED)
        (void) drivers_dma_mark_uncached(physBase, pageCount);

    /*
     * Fill the CPU PTE array. memdescGetPteArraySize returns 1 for a
     * physically-contiguous memdesc (single base entry) and PageCount for a
     * non-contiguous one (one entry per page); a contiguous block satisfies
     * both because entry i is simply base + i*RM_PAGE_SIZE.
     */
    pteArray = memdescGetPteArray(pMemDesc, AT_CPU);
    pteCount = memdescGetPteArraySize(pMemDesc, AT_CPU);
    for (i = 0; i < pteCount; i++)
        pteArray[i] = physBase + (NvU64) i * RM_PAGE_SIZE;

    /* Record alloc bookkeeping just like the real osAllocPagesInternal
     * (os_page_size == RM_PAGE_SIZE on x86_64, so no OS/RM page inflation). */
    status = memdescSetAllocSizeFields(pMemDesc, pageCount * RM_PAGE_SIZE, RM_PAGE_SIZE);
    if (status != NV_OK)
    {
        (void) drivers_dma_dealloc(physBase, pageCount);
        return status;
    }

    pAlloc = portMemAllocNonPaged(sizeof(*pAlloc));
    if (pAlloc == NULL)
    {
        (void) drivers_dma_dealloc(physBase, pageCount);
        return NV_ERR_NO_MEMORY;
    }
    pAlloc->physBase  = physBase;
    pAlloc->pageCount = pageCount;

    memdescSetMemData(pMemDesc, pAlloc, NULL);
    return NV_OK;
}

void osFreePagesInternal(MEMORY_DESCRIPTOR *pMemDesc)
{
    EclipseSysmemAlloc *pAlloc = (EclipseSysmemAlloc *) memdescGetMemData(pMemDesc);

    if (pAlloc == NULL)
        return;

    (void) drivers_dma_dealloc(pAlloc->physBase, pAlloc->pageCount);
    portMemFree(pAlloc);
    memdescSetMemData(pMemDesc, NULL, NULL);
}

NV_STATUS osMapSystemMemory(
    MEMORY_DESCRIPTOR *pMemDesc,
    NvU64              Offset,
    NvU64              Length,
    NvBool             Kernel,
    NvU32              Protect,
    NvP64             *ppAddress,
    NvP64             *ppPrivate
)
{
    NvU64               rootOffset = 0;
    MEMORY_DESCRIPTOR  *pRoot      = memdescGetRootMemDesc(pMemDesc, &rootOffset);
    EclipseSysmemAlloc *pAlloc;
    NvU64               va;

    (void) Protect;

    *ppAddress = NvP64_NULL;
    *ppPrivate = NvP64_NULL;

    if ((Offset + Length) < Length)
        return NV_ERR_INVALID_ARGUMENT;
    if ((Offset + Length) > memdescGetSize(pMemDesc))
        return NV_ERR_INVALID_ARGUMENT;

    /*
     * Only kernel mappings are supported: every RM-internal sysmem
     * allocation on the bring-up path maps with Kernel=NV_TRUE. A userspace
     * mapping would need a client VMA Eclipse's RM has no consumer for yet.
     */
    if (!Kernel)
        return NV_ERR_NOT_SUPPORTED;

    pAlloc = (EclipseSysmemAlloc *) memdescGetMemData(pRoot);
    if (pAlloc == NULL)
        return NV_ERR_INVALID_STATE;

    /* Eclipse maps all physical RAM in the kernel physmap, so a kernel VA is
     * just the physmap image of the block's physical base -- no per-mapping
     * page-table work, and thus nothing for osUnmapSystemMemory to unwind. */
    va = drivers_phys_to_virt(pAlloc->physBase + rootOffset + Offset);
    if (va == 0)
        return NV_ERR_GENERIC;

    *ppAddress = NV_PTR_TO_NvP64((void *) (NvUPtr) va);
    *ppPrivate = NvP64_NULL;
    return NV_OK;
}

void osUnmapSystemMemory(
    MEMORY_DESCRIPTOR *pMemDesc,
    NvBool             Kernel,
    NvU32              ProcessId,
    NvP64              pAddress,
    NvP64              pPrivate
)
{
    /* Physmap image -- the mapping is not a separate allocation, so there is
     * nothing to tear down here (the pages are released in
     * osFreePagesInternal). */
    (void) pMemDesc;
    (void) Kernel;
    (void) ProcessId;
    (void) pAddress;
    (void) pPrivate;
}

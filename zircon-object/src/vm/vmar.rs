use {
    super::*,
    crate::object::*,
    alloc::{collections::BTreeMap, sync::Arc, vec, vec::Vec},
    bitflags::bitflags,
    core::sync::atomic::{AtomicBool, AtomicU64, Ordering},
    kernel_hal::vm::{
        GenericPageTable, IgnoreNotMappedErr, Page, PageSize, PageTable, PagingError, PagingResult,
    },
    lock::Mutex,
};

/// Master switch for copy-on-write `fork` — on by default, disabled with
/// `FORKCOW=0` on the kernel command line.
///
/// On, `fork` hands the child a Zircon snapshot clone of each mapping's VMO
/// instead of copying every resident frame. Measured under QEMU against the
/// eager path: forking with 16 MiB resident went from 69.9 ms to 22.8 ms, and
/// the cost per resident MiB from 4.19x a `memcpy` of that MiB down to 0.59x
/// (Linux: 0.15x). `eclipse-bench`'s `fork memory isolation` check — parent and
/// child each verify the other's writes did not reach them — passes.
///
/// History. It was first shipped off because repeated `fork` of a process with
/// a pre-faulted anonymous region wedged the machine — a re-entrant self-deadlock
/// in `Drop for VmObject` that copy-on-write made reachable, since fixed. It was
/// then rolled back a SECOND time for a distinct defect: user-memory corruption
/// with a deterministic reproducer,
///
///   dd if=/dev/zero of=/tmp/z bs=4096 count=64 && md5sum /tmp/z
///   -> "malloc(): invalid next size (unsorted)", SIGABRT, every time.
///
/// glibc read a heap chunk header it never wrote: two processes writing one page
/// that should have been copied. That is now root-caused and fixed. The cause was
/// a stale WRITABLE TLB entry, not the COW logic: `protect_for_cow` write-protects
/// the parent's pages and the shootdown must invalidate every other CPU's TLB, but
/// a CPU spinning in a ticket lock has IRQs off and could not service the
/// shootdown IPI, while the initiator burned its ack budget and gave up. The
/// spinning CPU kept a writable TLB entry onto a frame the child now shared, and a
/// write through it landed on the child's page — exactly the corruption. This
/// session's shootdown spin-pump (`tlb_shootdown_pump`, drained every 512 spins in
/// the ticket-lock loop; see `kernel-hal`/`kernel-sync`) makes a spinning CPU
/// flush its own queue, so no writable entry survives the write-protect.
///
/// Re-validated on this build under `FORKCOW=1`, 4 vCPU: the exact reproducer and
/// heavier variants (20x serial md5, a 2 MiB random-file loop, 40-way PARALLEL
/// md5 — the SMP path most likely to expose the race) all return one identical
/// checksum; `fork memory isolation` PASSes; the login shell survives.
/// Measured win vs the eager path and vs Linux (same QEMU/TCG): see
/// docs/README-performance.md.
///
/// OFF BY DEFAULT AGAIN — third strike, and this time with the reproducer in
/// the tree (`tools/fork-hammer`). Its P7 pattern (fork storm while sibling
/// threads hot-write pages and churn the address space, the labwc/llvmpipe
/// shape) intermittently hands a fork child a page whose content is the
/// ORIGINAL mmap fill from thousands of writer passes earlier — verified
/// byte-exact against the computed pattern. Physically that frame can only be
/// the one a create_child captured before the writer's first store, served
/// later by the hidden-node snapshot tree instead of the current frame: a
/// resurrection somewhere in the collapse/merge machinery (`remove_child` /
/// `replace_child`), which survived both the process-wide mmap_lock and the
/// shootdown-ack watermark fix. Zircon-style hidden-node trees are exactly
/// the machinery Linux does not have — Linux forks with flat per-page COW and
/// refcounts, no tree, no collapse, nothing to resurrect. Until this VMO
/// layer is rebuilt that way, fork takes the EAGER path: `fork_copy` copies
/// only committed pages (file mappings stay lazy through `source`/`cache`),
/// no VMO ever gains a parent, and the entire fossil class is structurally
/// unreachable. `FORKCOW=1` re-enables the snapshot tree for experiments.
static COW_FORK: AtomicBool = AtomicBool::new(false);

/// Enable/disable copy-on-write `fork`. Called once at boot from the command
/// line.
pub fn set_cow_fork(enabled: bool) {
    COW_FORK.store(enabled, Ordering::Relaxed);
}

/// Whether copy-on-write `fork` is enabled.
pub fn cow_fork_enabled() -> bool {
    COW_FORK.load(Ordering::Relaxed)
}

/// Master switch for batching a fork's cross-CPU TLB shootdowns into one,
/// settable from the kernel command line (`FORKGATHER=0`). Off, every mapping
/// pays its own shootdown exactly as before — so the two behaviours can be
/// compared on one build, on one machine, by rebooting.
///
/// Only ever consulted for a single-threaded parent; see
/// [`VmAddressRegion::fork_from`] for why that condition is the whole basis of
/// the batching being sound.
static FORK_GATHER: AtomicBool = AtomicBool::new(true);

/// Enable/disable batching of a fork's TLB shootdowns.
pub fn set_fork_gather(enabled: bool) {
    FORK_GATHER.store(enabled, Ordering::Relaxed);
}

/// Whether a fork batches its TLB shootdowns into one.
pub fn fork_gather_enabled() -> bool {
    FORK_GATHER.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Fork phase accounting
// ---------------------------------------------------------------------------
//
// Time spent inside each step of cloning one mapping, so the cost of a fork can
// be attributed without a profiler. See `VmMapping::try_cow_child_timed` for
// what these were added to answer.
static FORK_NS_TOTAL: AtomicU64 = AtomicU64::new(0);
static FORK_NS_CREATE_CHILD: AtomicU64 = AtomicU64::new(0);
static FORK_NS_PROTECT: AtomicU64 = AtomicU64::new(0);
static FORK_NS_MAP_COMMITTED: AtomicU64 = AtomicU64::new(0);
static FORK_MAPPINGS: AtomicU64 = AtomicU64::new(0);
/// Heap allocations charged to `clone_map`, counted by sampling the global
/// allocation counter around it.
///
/// Attribution by sampling a global counter is only as good as the assumption
/// that nothing else allocates in between. A `fork` runs with the forking thread
/// blocked in the syscall and, in the measurements this exists for, the other
/// CPUs idle — so the error is whatever an interrupt handler allocates, which is
/// noise against the tens of allocations a single mapping costs. It buys a
/// per-caller allocation count with no plumbing through the allocator.
static FORK_ALLOCS: AtomicU64 = AtomicU64::new(0);
/// Mappings that could NOT be shared copy-on-write and were copied eagerly,
/// and the bytes those copies moved. One such mapping in a fork is enough to
/// dominate it: the copy is O(resident bytes) while everything else is O(1) per
/// mapping, so an average spread over hundreds of cheap mappings hides it.
static FORK_EAGER_MAPPINGS: AtomicU64 = AtomicU64::new(0);
static FORK_EAGER_BYTES: AtomicU64 = AtomicU64::new(0);
static FORK_NS_EAGER: AtomicU64 = AtomicU64::new(0);

/// Charges the whole of `clone_map` to `FORK_NS_TOTAL`, so the phases below can
/// be compared against it and whatever is left over is named.
struct ForkPhase {
    ns: u64,
    allocs: u64,
}

impl ForkPhase {
    fn start() -> Self {
        FORK_MAPPINGS.fetch_add(1, Ordering::Relaxed);
        ForkPhase {
            ns: kernel_hal::timer::timer_now().as_nanos() as u64,
            allocs: kernel_hal::kstats::heap_alloc_calls(),
        }
    }
}

impl Drop for ForkPhase {
    fn drop(&mut self) {
        let now = kernel_hal::timer::timer_now().as_nanos() as u64;
        FORK_NS_TOTAL.fetch_add(now.saturating_sub(self.ns), Ordering::Relaxed);
        FORK_ALLOCS.fetch_add(
            kernel_hal::kstats::heap_alloc_calls().saturating_sub(self.allocs),
            Ordering::Relaxed,
        );
    }
}

/// Per-mapping fork cost broken into phases, in nanoseconds:
/// `(mappings cloned, total, create_child, protect_for_cow, map_committed)`.
///
/// `total` covers all of `clone_map`; the difference between it and the three
/// phases is the eager-copy fallback plus bookkeeping.
pub fn fork_phase_stats() -> (u64, u64, u64, u64, u64, u64) {
    (
        FORK_MAPPINGS.load(Ordering::Relaxed),
        FORK_NS_TOTAL.load(Ordering::Relaxed),
        FORK_NS_CREATE_CHILD.load(Ordering::Relaxed),
        FORK_NS_PROTECT.load(Ordering::Relaxed),
        FORK_NS_MAP_COMMITTED.load(Ordering::Relaxed),
        FORK_ALLOCS.load(Ordering::Relaxed),
    )
}

/// Mappings a fork had to copy eagerly rather than share:
/// `(count, bytes, nanoseconds)`.
pub fn fork_eager_stats() -> (u64, u64, u64) {
    (
        FORK_EAGER_MAPPINGS.load(Ordering::Relaxed),
        FORK_EAGER_BYTES.load(Ordering::Relaxed),
        FORK_NS_EAGER.load(Ordering::Relaxed),
    )
}

/// Charge `ns` to the `map_committed` phase. Called from `fork_from`, which is
/// where that step happens.
fn note_map_committed(ns: u64) {
    FORK_NS_MAP_COMMITTED.fetch_add(ns, Ordering::Relaxed);
}

/// Charges the eager-copy fallback, however it exits.
struct EagerPhase(u64);

impl Drop for EagerPhase {
    fn drop(&mut self) {
        let now = kernel_hal::timer::timer_now().as_nanos() as u64;
        FORK_NS_EAGER.fetch_add(now.saturating_sub(self.0), Ordering::Relaxed);
    }
}

bitflags! {
    /// Creation flags for VmAddressRegion.
    pub struct VmarFlags: u32 {
        #[allow(clippy::identity_op)]
        /// When randomly allocating subregions, reduce sprawl by placing allocations
        /// near each other.
        const COMPACT               = 1 << 0;
        /// Request that the new region be at the specified offset in its parent region.
        const SPECIFIC              = 1 << 1;
        /// Like SPECIFIC, but permits overwriting existing mappings.  This
        /// flag will not overwrite through a subregion.
        const SPECIFIC_OVERWRITE    = 1 << 2;
        /// Allow VmMappings to be created inside the new region with the SPECIFIC or
        /// OFFSET_IS_UPPER_LIMIT flag.
        const CAN_MAP_SPECIFIC      = 1 << 3;
        /// Allow VmMappings to be created inside the region with read permissions.
        const CAN_MAP_READ          = 1 << 4;
        /// Allow VmMappings to be created inside the region with write permissions.
        const CAN_MAP_WRITE         = 1 << 5;
        /// Allow VmMappings to be created inside the region with execute permissions.
        const CAN_MAP_EXECUTE       = 1 << 6;
        /// Require that VMO backing the mapping is non-resizable.
        const REQUIRE_NON_RESIZABLE = 1 << 7;
        /// Treat the offset as an upper limit when allocating a VMO or child VMAR.
        const ALLOW_FAULTS          = 1 << 8;

        /// Allow VmMappings to be created inside the region with read, write and execute permissions.
        const CAN_MAP_RXW           = Self::CAN_MAP_READ.bits | Self::CAN_MAP_EXECUTE.bits | Self::CAN_MAP_WRITE.bits;
        /// Creation flags for root VmAddressRegion
        const ROOT_FLAGS            = Self::CAN_MAP_RXW.bits | Self::CAN_MAP_SPECIFIC.bits;
    }
}

/// Virtual Memory Address Regions
pub struct VmAddressRegion {
    flags: VmarFlags,
    base: KObjectBase,
    _counter: CountHelper,
    addr: VirtAddr,
    size: usize,
    parent: Option<Arc<VmAddressRegion>>,
    page_table: Arc<Mutex<dyn GenericPageTable>>,
    /// If inner is None, this region is destroyed, all operations are invalid.
    inner: Mutex<Option<VmarInner>>,
}

impl_kobject!(VmAddressRegion);
define_count_helper!(VmAddressRegion);

/// The mutable part of `VmAddressRegion`.
#[derive(Default)]
struct VmarInner {
    children: Vec<Arc<VmAddressRegion>>,
    /// Live mappings of this region, keyed by their start address.
    ///
    /// Was a `Vec` scanned linearly by every consumer — `test_map`,
    /// `find_mapping`, `handle_page_fault`, `determine_offset` — which turned
    /// each `mmap` into an O(n) walk and a process that maps thousands of
    /// anonymous regions (Mesa under labwc) into O(n²). Mappings never overlap,
    /// so their start address is a total order consistent with their ranges:
    /// keyed this way, "which mapping contains `vaddr`" and "does `[a, b)`
    /// overlap anything" are O(log n) range queries, and — because the highest
    /// start is also the highest end — "where does the next mapping go" is the
    /// map's last entry. The move-to-front hack the linear scans grew is gone;
    /// the tree subsumes it.
    mappings: BTreeMap<VirtAddr, Arc<VmMapping>>,
    /// Ring buffer of the most recent ranges removed from this VMAR, for the
    /// open "userspace faults on memory mmap successfully installed" bug.
    ///
    /// Established by experiment: on a crashing boot neither [mmap-lost] nor
    /// [vmar-corrupt] fires, so mmap DID install the range and the mappings are
    /// structurally sound — the region is created correctly and then destroyed
    /// while userspace still believes it owns it. This records who destroyed
    /// what, so the fault dump can name the unmap that removed the address
    /// instead of only showing that it is gone.
    ///
    /// `(addr, len, reason)`; 16 entries, oldest overwritten. A FIXED ARRAY,
    /// deliberately not a `Vec`: this is written under the VMAR's spinlock with
    /// interrupts off, on the unmap path, and a `Vec` push can call the global
    /// allocator there — allocating under an IRQ-off spinlock in a hot kernel
    /// path is exactly the kind of thing that turns a diagnostic into a bug.
    recent_unmaps: [(VirtAddr, usize, &'static str); UNMAP_HISTORY],
    /// Next slot to write in `recent_unmaps` (wraps).
    unmap_head: usize,
}

/// How many recent unmaps each VMAR remembers (see `VmarInner::recent_unmaps`).
const UNMAP_HISTORY: usize = 16;

impl VmAddressRegion {
    /// Create a new root VMAR.
    pub fn new_root() -> Arc<Self> {
        #[cfg(feature = "aspace-separate")]
        let (addr, size) = {
            use core::sync::atomic::*;
            static VMAR_ID: AtomicUsize = AtomicUsize::new(0);
            let i = VMAR_ID.fetch_add(1, Ordering::SeqCst);
            (0x2_0000_0000 + 0x100_0000_0000 * i, 0x100_0000_0000)
        };
        #[cfg(not(feature = "aspace-separate"))]
        let (addr, size) = (USER_ASPACE_BASE as usize, USER_ASPACE_SIZE as usize);
        Arc::new(VmAddressRegion {
            flags: VmarFlags::ROOT_FLAGS,
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            addr,
            size,
            parent: None,
            page_table: Arc::new(Mutex::new(PageTable::from_current().clone_kernel())), //hal PageTable
            inner: Mutex::new(Some(VmarInner::default())),
        })
    }

    /// Create a kernel root VMAR.
    pub fn new_kernel() -> Arc<Self> {
        let kernel_vmar_base = KERNEL_ASPACE_BASE as usize; // Sorry i hard code because i'm lazy
        let kernel_vmar_size = KERNEL_ASPACE_SIZE as usize;
        Arc::new(VmAddressRegion {
            flags: VmarFlags::ROOT_FLAGS,
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            addr: kernel_vmar_base,
            size: kernel_vmar_size,
            parent: None,
            page_table: Arc::new(Mutex::new(PageTable::from_current().clone_kernel())),
            inner: Mutex::new(Some(VmarInner::default())),
        })
    }

    /// Create a VMAR for guest physical memory.
    #[cfg(feature = "hypervisor")]
    pub fn new_guest() -> Arc<Self> {
        let guest_vmar_base = crate::hypervisor::GUEST_PHYSICAL_ASPACE_BASE as usize;
        let guest_vmar_size = crate::hypervisor::GUEST_PHYSICAL_ASPACE_SIZE as usize;
        Arc::new(VmAddressRegion {
            flags: VmarFlags::ROOT_FLAGS,
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            addr: guest_vmar_base,
            size: guest_vmar_size,
            parent: None,
            page_table: Arc::new(Mutex::new(crate::hypervisor::VmmPageTable::new())),
            inner: Mutex::new(Some(VmarInner::default())),
        })
    }

    /// Create a child VMAR at the `offset`.
    pub fn allocate_at(
        self: &Arc<Self>,
        offset: usize,
        len: usize,
        flags: VmarFlags,
        align: usize,
    ) -> ZxResult<Arc<Self>> {
        self.allocate(Some(offset), len, flags, align)
    }

    /// Create a child VMAR with optional `offset`.
    pub fn allocate(
        self: &Arc<Self>,
        offset: Option<usize>,
        len: usize,
        flags: VmarFlags,
        align: usize,
    ) -> ZxResult<Arc<Self>> {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        // No floor: the ELF loader's app sub-VMAR must land at offset 0 for
        // non-PIE binaries (see determine_offset).
        let offset = self.determine_offset(inner, offset, len, align, 0)?;
        let child = Arc::new(VmAddressRegion {
            flags,
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            addr: self.addr + offset,
            size: len,
            parent: Some(self.clone()),
            page_table: self.page_table.clone(),
            inner: Mutex::new(Some(VmarInner::default())),
        });
        inner.children.push(child.clone());
        Ok(child)
    }

    /// Map the `vmo` into this VMAR at given `offset`.
    pub fn map_at(
        &self,
        vmar_offset: usize,
        vmo: Arc<VmObject>,
        vmo_offset: usize,
        len: usize,
        flags: MMUFlags,
    ) -> ZxResult<VirtAddr> {
        self.map(Some(vmar_offset), vmo, vmo_offset, len, flags)
    }

    /// Map the `vmo` into this VMAR.
    pub fn map(
        &self,
        vmar_offset: Option<usize>,
        vmo: Arc<VmObject>,
        vmo_offset: usize,
        len: usize,
        flags: MMUFlags,
    ) -> ZxResult<VirtAddr> {
        self.map_ext(
            vmar_offset,
            vmo,
            vmo_offset,
            len,
            MMUFlags::RXW,
            flags,
            false,
            true,
        )
    }

    /// Map the `vmo` into this VMAR.
    #[allow(clippy::too_many_arguments)]
    pub fn map_ext(
        &self,
        vmar_offset: Option<usize>,
        vmo: Arc<VmObject>,
        vmo_offset: usize,
        len: usize,
        permissions: MMUFlags,
        flags: MMUFlags,
        overwrite: bool,
        map_range: bool,
    ) -> ZxResult<VirtAddr> {
        self.map_ext_min(
            vmar_offset,
            vmo,
            vmo_offset,
            len,
            permissions,
            flags,
            overwrite,
            map_range,
            0,
        )
    }

    /// Like [`map_ext`](Self::map_ext), with a `min_offset` floor for
    /// kernel-chosen placement. The Linux mmap path passes `mmap_min_addr`
    /// (64 KiB) here so a non-FIXED `mmap(NULL, ...)` can never be placed at
    /// address 0: userspace treats the returned 0 as a valid pointer, then
    /// every syscall null-check rejects it with EFAULT (observed as
    /// `dd bs=4096` failing "Bad address" — its freshly mmap'd I/O buffer was
    /// at 0x0). The floor also restores NULL-dereference protection.
    /// Explicit placements (`vmar_offset = Some(..)`, e.g. MAP_FIXED) are
    /// unaffected.
    #[allow(clippy::too_many_arguments)]
    pub fn map_ext_min(
        &self,
        vmar_offset: Option<usize>,
        vmo: Arc<VmObject>,
        vmo_offset: usize,
        len: usize,
        permissions: MMUFlags,
        flags: MMUFlags,
        overwrite: bool,
        map_range: bool,
        min_offset: usize,
    ) -> ZxResult<VirtAddr> {
        // `len == 0` must be rejected, not just for Linux/Zircon semantics: the
        // mappings tree is keyed by start address, and inserting a zero-length
        // mapping at an explicit offset equal to an existing mapping's start
        // would REPLACE that live mapping in the BTreeMap (its Drop unmaps its
        // pages) — a zero-size interval overlaps nothing, so `test_map` cannot
        // catch it. No current caller passes 0 (sys_mmap/mremap reject it,
        // loader/vDSO sizes are fixed nonzero); this keeps it that way.
        if len == 0
            || !page_aligned(vmo_offset)
            || !page_aligned(len)
            || vmo_offset.overflowing_add(len).1
        {
            return Err(ZxError::INVALID_ARGS);
        }
        if !permissions.contains(flags & MMUFlags::RXW) {
            return Err(ZxError::ACCESS_DENIED);
        }
        // TODO: allow the mapping extends past the end of vmo
        if vmo_offset > vmo.len() || len > vmo.len() - vmo_offset {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        // `determine_offset` refuses an explicit offset whose range is not FREE
        // (`test_map`) and reports INVALID_ARGS. That made `overwrite` dead code
        // for explicit placement: the range being occupied is precisely the case
        // `overwrite` exists for, yet the request never survived long enough to
        // reach the overwrite branch below. Observed as every MAP_FIXED segment
        // of every shared library failing —
        //   Error loading shared library libX11.so.6: Invalid argument
        // — because musl's ldso reserves the image with one PROT_NONE mapping
        // and then MAP_FIXEDs each segment over it.
        //
        // So for `Some(offset) + overwrite`, validate only what still applies
        // (alignment and bounds); occupancy is handled a few lines down by
        // `unmap_inner_why`, which carves the hole under this same lock right
        // before the replacement is inserted.
        let offset = match vmar_offset {
            Some(offset) if overwrite => {
                if !check_aligned(offset, PAGE_SIZE)
                    || !check_aligned(len, PAGE_SIZE)
                    || offset.checked_add(len).is_none_or(|end| end > self.size)
                {
                    return Err(ZxError::INVALID_ARGS);
                }
                offset
            }
            _ => self.determine_offset(inner, vmar_offset, len, PAGE_SIZE, min_offset)?,
        };
        let addr = self.addr + offset;
        let mut flags = flags;
        // if vmo != 0
        {
            flags |= MMUFlags::from_bits_truncate(vmo.cache_policy() as usize);
        }
        // align = 1K? 2K? 4K? 8K? ...
        if !self.test_map(inner, offset, len, PAGE_SIZE) {
            if overwrite {
                self.unmap_inner_why(addr, len, inner, "map_ext(overwrite)")?;
            } else {
                return Err(ZxError::NO_MEMORY);
            }
        }
        // Respect the caller's `map_range`. The historical upstream-zCore
        // workaround here (`map_range || vmo.name() != ""`) force-committed
        // every NAMED vmo at map time — and since every file-backed VMO is
        // named with its path (fs/file.rs `set_name`), it silently disabled
        // demand paging for every executable and library mapping: an mmap of a
        // 30 MiB .so performed ~7700 synchronous 4 KiB reads inside the
        // syscall, under the IRQ-off VMAR lock. Callers that genuinely need
        // eager mapping (kernel aspace loads via `map()`, the vDSO) already
        // pass `map_range = true` explicitly; faults on file pages resolve
        // through `FrameFiller::fill_page` exactly as the eager path did.
        let mapping = VmMapping::new(
            addr,
            len,
            vmo,
            vmo_offset,
            permissions,
            flags,
            self.page_table.clone(),
        );
        if map_range {
            mapping.map()?;
        }
        inner.mappings.insert(mapping.addr(), mapping);
        Ok(addr)
    }

    /// Unmaps all VMO mappings and destroys all sub-regions within the absolute range
    /// including `addr` and ending before exclusively at `addr + len`.
    /// Any sub-region that is in the range must be fully in the range
    /// (i.e. partial overlaps are an error).
    /// If a mapping is only partially in the range, the mapping is split and the requested
    /// portion is unmapped.
    pub fn unmap(&self, addr: VirtAddr, len: usize) -> ZxResult {
        self.unmap_why(addr, len, "unmap")
    }

    /// `unmap` with a caller tag recorded in the unmap history, so a later
    /// fault dump names WHICH operation removed an address rather than a
    /// generic "unmap" (see `VmarInner::recent_unmaps`).
    pub fn unmap_why(&self, addr: VirtAddr, len: usize, why: &'static str) -> ZxResult {
        if !page_aligned(addr) || !page_aligned(len) || len == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        self.unmap_inner_why(addr, len, inner, why)
    }

    /// Must hold self.inner.lock() before calling.
    /// `unmap_inner` plus a tag recorded in the unmap history, so a later fault
    /// dump can say WHICH operation removed the address (see
    /// `VmarInner::recent_unmaps`).
    fn unmap_inner_why(
        &self,
        addr: VirtAddr,
        len: usize,
        inner: &mut VmarInner,
        why: &'static str,
    ) -> ZxResult {
        if !page_aligned(addr) || !page_aligned(len) || len == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        // Allocation-free ring write (see `VmarInner::recent_unmaps`).
        inner.recent_unmaps[inner.unmap_head] = (addr, len, why);
        inner.unmap_head = (inner.unmap_head + 1) % UNMAP_HISTORY;

        let begin = addr;
        let end = addr + len;
        // check partial overlapped sub-regions
        for vmar in inner.children.iter() {
            if vmar.partial_overlap(begin, end) {
                return Err(ZxError::INVALID_ARGS);
            }
        }
        // Touch only the mappings that actually overlap [begin, end) — an
        // O(k + log n) range query — instead of taking and rebuilding the whole
        // list on every unmap. Each overlapping mapping is removed first, then
        // `cut`; `cut`'s prefix case moves the start address, so the survivor is
        // reinserted under its (possibly new) key, and any split-off tail is
        // inserted at its own start. Non-overlap of the result guarantees no key
        // collision: at most one mapping straddles `end`, so at most one
        // reinsertion lands at `end`.
        let mut new_maps = Vec::new();
        for key in Self::overlapping_keys(inner, begin, end) {
            let map = inner
                .mappings
                .remove(&key)
                .expect("overlapping key must be present");
            if let Some(new) = map.cut(begin, end) {
                new_maps.push(new);
            }
            if map.size() > 0 {
                inner.mappings.insert(map.addr(), map);
            }
        }
        for new in new_maps {
            inner.mappings.insert(new.addr(), new);
        }
        for vmar in core::mem::take(&mut inner.children) {
            if vmar.within(begin, end) {
                vmar.destroy_internal()?;
            } else {
                inner.children.push(vmar);
            }
        }
        Ok(())
    }

    /// Change protections on a subset of the region of memory in the containing
    /// address space.
    ///
    /// Zircon-style VMARs historically rejected ranges that overlap subregions,
    /// but Linux `mprotect()` routinely targets ranges that may span multiple
    /// mapped segments/sub-VMARs (e.g. PIE + vDSO layouts). So we treat a VMAR
    /// hierarchy as a single address space: protect any overlapping mappings
    /// in this VMAR and recurse into overlapping children.
    pub fn protect(&self, addr: usize, len: usize, flags: MMUFlags) -> ZxResult {
        if !page_aligned(addr) || !page_aligned(len) || len == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let end_addr = addr + len;

        // Collect work items under the VMAR lock, then apply without holding it.
        let (children, mappings) = {
            let mut guard = self.inner.lock();
            let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;

            let children: Vec<_> = inner
                .children
                .iter()
                .filter(|child| child.end_addr() > addr && child.addr() < end_addr)
                .cloned()
                .collect();

            let mappings: Vec<_> = inner
                .mappings
                .values()
                .filter(|map| map.end_addr() > addr && map.addr() < end_addr)
                .cloned()
                .collect();

            // Ensure the range is fully covered by mappings and/or children.
            let covered_by_mappings: usize = mappings
                .iter()
                .map(|map| end_addr.min(map.end_addr()) - addr.max(map.addr()))
                .sum();
            let covered_by_children: usize = children
                .iter()
                .map(|child| end_addr.min(child.end_addr()) - addr.max(child.addr()))
                .sum();
            if covered_by_mappings + covered_by_children != len {
                return Err(ZxError::NOT_FOUND);
            }

            // Validate flags against mapping permissions (children validate themselves).
            if mappings
                .iter()
                .any(|map| !map.is_valid_mapping_flags(flags))
            {
                return Err(ZxError::ACCESS_DENIED);
            }

            (children, mappings)
        };

        // Apply to children first.
        for child in children {
            let begin = addr.max(child.addr());
            let end = end_addr.min(child.end_addr());
            if begin < end {
                child.protect(begin, end - begin, flags)?;
            }
        }

        // Apply to our own mappings.
        for map in mappings {
            let begin = addr.max(map.addr());
            let end = end_addr.min(map.end_addr());
            if begin < end {
                let start_index = pages(begin - map.addr());
                let end_index = pages(end - map.addr());
                map.protect(flags, start_index, end_index);
            }
        }

        Ok(())
    }

    /// `madvise(MADV_DONTNEED)` — discard the committed pages in `[addr, addr+len)`
    /// so the next access reads back as zero (anonymous) / re-reads from source
    /// (file-backed private).
    ///
    /// Page-table-walk zeroing is NOT enough: apk's mimalloc decommits an arena
    /// region (its PTEs get dropped / turned PROT_NONE) and reuses it later; the
    /// frame is still owned by the VMO, so the reuse fault hands that STALE frame
    /// back and mimalloc reads old bytes where it assumes fresh OS zeros, aborting
    /// with "corrupted free list entry". The fix is to zero those frames THROUGH
    /// THE VMO (`committed_paddr`), which reaches them regardless of PTE state,
    /// while leaving the mapping intact.
    pub fn madv_dontneed(&self, addr: VirtAddr, len: usize) {
        if !page_aligned(addr) || len == 0 {
            return;
        }
        let end = addr + len;
        let (mappings, children) = {
            let guard = self.inner.lock();
            let inner = match guard.as_ref() {
                Some(i) => i,
                None => return,
            };
            let m: Vec<_> = inner
                .mappings
                .values()
                .filter(|map| map.end_addr() > addr && map.addr() < end)
                .cloned()
                .collect();
            let c: Vec<_> = inner
                .children
                .iter()
                .filter(|ch| ch.end_addr() > addr && ch.addr() < end)
                .cloned()
                .collect();
            (m, c)
        };
        for map in mappings {
            map.dontneed(addr.max(map.addr()), end.min(map.end_addr()));
        }
        for child in children {
            let cb = addr.max(child.addr());
            let ce = end.min(child.end_addr());
            child.madv_dontneed(cb, ce - cb);
        }
    }

    /// Linux `mremap(2)`: resize the mapping containing `[old_addr, old_addr+old_len)`,
    /// moving it when necessary (and permitted via `may_move`) or to the exact
    /// address requested with `fixed` (MREMAP_FIXED). Returns the new base address.
    ///
    /// The whole old range must lie inside ONE existing mapping (Linux demands a
    /// single VMA and answers EFAULT otherwise → `ZxError::NOT_FOUND` here).
    /// Contents are preserved without copying in every path: the moved range is
    /// re-mapped from the SAME VmObject window (frames travel with the VMO), and
    /// growth beyond the VMO's tail is satisfied by an adjacent fresh anonymous
    /// demand-paged mapping that reads back as zero — exactly what mremap
    /// guarantees for grown anonymous memory. Keeping the VMO also keeps
    /// MAP_SHARED semantics: other mappers of the object still see the stores.
    ///
    /// All decisions and mutations happen under one take of the VMAR lock, so a
    /// concurrent mmap/munmap cannot claim the chosen target range in between
    /// (the userspace-visible atomicity mremap has on Linux via mmap_lock).
    pub fn remap(
        &self,
        old_addr: VirtAddr,
        old_len: usize,
        new_len: usize,
        may_move: bool,
        fixed: Option<VirtAddr>,
    ) -> ZxResult<VirtAddr> {
        if !page_aligned(old_addr)
            || !page_aligned(old_len)
            || !page_aligned(new_len)
            || old_len == 0
            || new_len == 0
        {
            return Err(ZxError::INVALID_ARGS);
        }
        let old_end = old_addr.checked_add(old_len).ok_or(ZxError::INVALID_ARGS)?;
        if let Some(new_addr) = fixed {
            // MREMAP_FIXED: target must be aligned and must not overlap the old
            // range (Linux returns EINVAL for both).
            let new_end = new_addr.checked_add(new_len).ok_or(ZxError::INVALID_ARGS)?;
            if !page_aligned(new_addr) || new_addr < self.addr || new_end > self.end_addr() {
                return Err(ZxError::INVALID_ARGS);
            }
            if new_addr < old_end && old_addr < new_end {
                return Err(ZxError::INVALID_ARGS);
            }
        }

        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;

        // The old range must be fully covered by a single mapping at this level.
        let mapping = Self::mapping_containing(inner, old_addr).ok_or(ZxError::NOT_FOUND)?;
        let (map_addr, map_size, map_vmo_offset, moved_flags) = {
            let m = mapping.inner.lock();
            if old_end > m.end_addr() {
                return Err(ZxError::NOT_FOUND);
            }
            // Per-page flags of the moved window, so protection state applied by
            // an earlier mprotect over a sub-range survives the move.
            let first = (old_addr - m.addr) / PAGE_SIZE;
            let flags: Vec<MMUFlags> = m.flags[first..first + pages(old_len)].to_vec();
            (m.addr, m.size, m.vmo_offset, flags)
        };
        let vmo = mapping.vmo.clone();
        let permissions = mapping.permissions;
        let vmo_off_old = map_vmo_offset + (old_addr - map_addr);
        let tail_flag = *moved_flags.last().unwrap();

        // How much of the new length the existing VMO can back from the old
        // window onward. For anonymous mappings the VMO ends with the mapping
        // (window == old_len); a shared file VMO may extend further, in which
        // case the grown part keeps showing file content like Linux does.
        let vmo_window = (vmo.len() - vmo_off_old).min(new_len);

        if fixed.is_none() {
            // Shrink (or no-op): drop the tail in place. Linux never moves here.
            if new_len <= old_len {
                if new_len < old_len {
                    self.unmap_inner_why(
                        old_addr + new_len,
                        old_len - new_len,
                        inner,
                        "mremap_shrink",
                    )?;
                }
                return Ok(old_addr);
            }

            // Grow in place when the old range is the tail of its mapping and
            // the address space right after it is free.
            let delta = new_len - old_len;
            let in_place_ok = old_end == map_addr + map_size
                && old_end + delta <= self.end_addr()
                && self.test_map(inner, old_end - self.addr, delta, PAGE_SIZE);
            if in_place_ok {
                if vmo_window >= new_len {
                    // The VMO already covers the extension: stretch the mapping.
                    let mut m = mapping.inner.lock();
                    m.size += delta;
                    m.flags
                        .extend(core::iter::repeat_n(tail_flag, pages(delta)));
                } else {
                    // Anonymous tail, demand-paged: zero on first touch.
                    let tail_vmo = VmObject::new_paged(pages(delta));
                    let tail = VmMapping::new(
                        old_end,
                        delta,
                        tail_vmo,
                        0,
                        permissions,
                        tail_flag,
                        self.page_table.clone(),
                    );
                    inner.mappings.insert(tail.addr(), tail);
                }
                return Ok(old_addr);
            }

            if !may_move {
                return Err(ZxError::NO_MEMORY);
            }
        }

        // Move: reserve the target range, re-map the same VMO window there, add
        // an anonymous tail for growth past the VMO, then drop the old range.
        let target_offset = match fixed {
            Some(new_addr) => {
                // MREMAP_FIXED unmaps whatever occupied the target, like MAP_FIXED.
                self.unmap_inner_why(new_addr, new_len, inner, "mremap_fixed_target")?;
                let offset = new_addr - self.addr;
                if !self.test_map(inner, offset, new_len, PAGE_SIZE) {
                    return Err(ZxError::NO_MEMORY);
                }
                offset
            }
            None => self
                .find_free_area(inner, 0, new_len, PAGE_SIZE)
                .ok_or(ZxError::NO_MEMORY)?,
        };
        let new_addr = self.addr + target_offset;

        let moved = VmMapping::new(
            new_addr,
            vmo_window,
            vmo,
            vmo_off_old,
            permissions,
            tail_flag,
            self.page_table.clone(),
        );
        {
            // Preserve the per-page protection state of the moved window; pages
            // the VMO backs beyond the old window (file growth) take the tail's.
            // A FIXED shrink moves FEWER pages than the old range had, so the
            // vector is first cut down to the window before padding it out.
            let mut m = moved.inner.lock();
            let window_pages = pages(vmo_window);
            let extra = window_pages.saturating_sub(moved_flags.len());
            m.flags = moved_flags;
            m.flags.truncate(window_pages);
            m.flags.extend(core::iter::repeat_n(tail_flag, extra));
        }
        // Install PTEs for the frames that already exist so the caller's
        // immediately-following accesses (memcpy into a realloc'd buffer) do not
        // pay one page fault each; untouched pages keep demand-faulting exactly
        // like they did at the old address. Same approach as the fork path.
        moved.map_committed()?;
        inner.mappings.insert(moved.addr(), moved);
        if vmo_window < new_len {
            let tail_vmo = VmObject::new_paged(pages(new_len - vmo_window));
            let tail = VmMapping::new(
                new_addr + vmo_window,
                new_len - vmo_window,
                tail_vmo,
                0,
                permissions,
                tail_flag,
                self.page_table.clone(),
            );
            inner.mappings.insert(tail.addr(), tail);
        }
        self.unmap_inner_why(old_addr, old_len, inner, "mremap_move_old")?;
        Ok(new_addr)
    }

    /// Snapshot every live mapping in this VMAR tree, sorted by start address —
    /// the data `/proc/<pid>/maps` renders. Geometry and the backing object's
    /// identity are read under the locks; the rows themselves are plain data.
    pub fn mappings_dump(&self) -> Vec<MappingDump> {
        let mut out = Vec::new();
        self.collect_mappings(&mut out);
        out.sort_by_key(|m| m.start);
        out
    }

    fn collect_mappings(&self, out: &mut Vec<MappingDump>) {
        let children: Vec<_> = {
            let guard = self.inner.lock();
            let inner = match guard.as_ref() {
                Some(i) => i,
                None => return,
            };
            for map in inner.mappings.values() {
                let m = map.inner.lock();
                if m.size == 0 {
                    continue;
                }
                // Linux VMAs have uniform protection; after a partial mprotect
                // split this kernel keeps per-page flags in one mapping, so the
                // leading page stands for the row.
                let flags = match m.flags.first() {
                    Some(f) => *f,
                    None => continue,
                };
                out.push(MappingDump {
                    start: m.addr,
                    end: m.end_addr(),
                    flags,
                    vmo_offset: m.vmo_offset,
                    shared: map.vmo.share_count() > 1,
                    vmo_id: map.vmo.id(),
                    name: map.vmo.name(),
                });
            }
            inner.children.to_vec()
        };
        for child in children {
            child.collect_mappings(out);
        }
    }

    /// Unmap all mappings within the VMAR, and destroy all sub-regions of the region.
    pub fn destroy(self: &Arc<Self>) -> ZxResult {
        self.destroy_internal()?;
        // remove from parent
        if let Some(parent) = &self.parent {
            let mut guard = parent.inner.lock();
            let inner = guard.as_mut().unwrap();
            inner.children.retain(|vmar| !Arc::ptr_eq(self, vmar));
        }
        Ok(())
    }

    /// Destroy but do not remove self from parent.
    fn destroy_internal(&self) -> ZxResult {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        for vmar in inner.children.drain(..) {
            vmar.destroy_internal()?;
        }
        inner.mappings.clear();
        *guard = None;
        Ok(())
    }

    /// Unmap all mappings and destroy all sub-regions of VMAR.
    pub fn clear(&self) -> ZxResult {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        for vmar in inner.children.drain(..) {
            vmar.destroy_internal()?;
        }
        inner.mappings.clear();
        Ok(())
    }

    /// Get physical address of the underlying page table.
    pub fn table_phys(&self) -> PhysAddr {
        self.page_table.lock().table_phys()
    }

    /// Get start address of this VMAR.
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// Whether this VMAR is dead.
    pub fn is_dead(&self) -> bool {
        self.inner.lock().is_none()
    }

    /// Whether this VMAR is alive.
    pub fn is_alive(&self) -> bool {
        !self.is_dead()
    }

    /// Get flags of vaddr
    pub fn get_vaddr_flags(&self, vaddr: usize) -> PagingResult<MMUFlags> {
        let guard = self.inner.lock();
        let inner = guard.as_ref().unwrap();
        if !self.contains(vaddr) {
            return Err(PagingError::NotMapped);
        }
        if let Some(child) = inner.children.iter().find(|ch| ch.contains(vaddr)) {
            return child.get_vaddr_flags(vaddr);
        }
        if let Some(mapping) = Self::mapping_containing(inner, vaddr) {
            return mapping.query_vaddr(vaddr).map(|(_, flags, _)| flags);
        }
        Err(PagingError::NoMemory)
    }

    /// The mapping in THIS region that contains `vaddr`, in O(log n).
    ///
    /// Mappings are keyed by start address and never overlap, so the only
    /// candidate is the one with the greatest start `<= vaddr`; it contains
    /// `vaddr` iff its end is above `vaddr`. Children are not consulted here —
    /// callers walk the child tree separately.
    fn mapping_containing(inner: &VmarInner, vaddr: VirtAddr) -> Option<Arc<VmMapping>> {
        inner
            .mappings
            .range(..=vaddr)
            .next_back()
            .filter(|(_, m)| m.contains(vaddr))
            .map(|(_, m)| m.clone())
    }

    /// Whether any mapping in THIS region overlaps `[begin, end)`, in O(log n)
    /// plus the (tiny) number of mappings that actually straddle the range.
    ///
    /// Because mappings are non-overlapping, the only one that can start at or
    /// before `begin` yet reach into the range is the entry at-or-before
    /// `begin`; every other overlapper starts within `[begin, end)`. Walk from
    /// that lower bound and stop at the first true overlap.
    fn any_mapping_overlaps(inner: &VmarInner, begin: VirtAddr, end: VirtAddr) -> bool {
        let lower = inner
            .mappings
            .range(..=begin)
            .next_back()
            .map(|(k, _)| *k)
            .unwrap_or(begin);
        inner
            .mappings
            .range(lower..end)
            .any(|(_, m)| m.end_addr() > begin)
    }

    /// Start-address keys of every mapping in THIS region overlapping
    /// `[begin, end)` (same reasoning as [`any_mapping_overlaps`]). Returned as
    /// keys, not clones, because the caller removes them from the map before
    /// cutting — `cut` can change a mapping's start address, which would leave a
    /// stale key behind if it were not removed first.
    fn overlapping_keys(inner: &VmarInner, begin: VirtAddr, end: VirtAddr) -> Vec<VirtAddr> {
        let lower = inner
            .mappings
            .range(..=begin)
            .next_back()
            .map(|(k, _)| *k)
            .unwrap_or(begin);
        inner
            .mappings
            .range(lower..end)
            .filter(|(_, m)| m.end_addr() > begin)
            .map(|(k, _)| *k)
            .collect()
    }

    /// Determine final address with given input `offset` and `len`.
    /// `min_offset` floors kernel-chosen (non-fixed) placement only; explicit
    /// `offset` requests are unaffected. See the comment in the else-branch.
    fn determine_offset(
        &self,
        inner: &VmarInner,
        offset: Option<usize>,
        len: usize,
        align: usize,
        min_offset: usize,
    ) -> ZxResult<VirtAddr> {
        if !check_aligned(len, align) {
            Err(ZxError::INVALID_ARGS)
        } else if let Some(offset) = offset {
            if check_aligned(offset, align) && self.test_map(inner, offset, len, align) {
                Ok(offset)
            } else {
                Err(ZxError::INVALID_ARGS)
            }
        } else if len > self.size {
            Err(ZxError::INVALID_ARGS)
        } else {
            // `min_offset` is a floor for kernel-chosen placement, used by the
            // Linux mmap path to enforce `mmap_min_addr` (see `map_ext`). It
            // MUST stay 0 for every other caller: the ELF loader's app
            // sub-VMAR (`allocate(None, ...)`) relies on landing at offset 0 —
            // a non-PIE binary's segments carry absolute vaddrs (0x400000+),
            // and floor-shifting that sub-VMAR shifts the whole image and
            // SIGSEGVs every process (observed when the floor was mistakenly
            // applied here globally).
            let floor = min_offset.next_multiple_of(align.max(1));
            self.find_free_area(inner, floor, len, align)
                .ok_or(ZxError::NO_MEMORY)
        }
    }

    /// Test if can create a new mapping at `offset` with `len`.
    fn test_map(&self, inner: &VmarInner, offset: usize, len: usize, align: usize) -> bool {
        debug_assert!(check_aligned(offset, align));
        debug_assert!(check_aligned(len, align));
        let begin = self.addr + offset;
        let end = begin + len;
        if end > self.addr + self.size {
            return false;
        }
        // Children are few (the ELF loader creates a handful of sub-VMARs), so a
        // linear scan of them is fine; the mapping check that used to dominate is
        // now an O(log n) range probe.
        if inner.children.iter().any(|vmar| vmar.overlap(begin, end)) {
            return false;
        }
        if Self::any_mapping_overlaps(inner, begin, end) {
            return false;
        }
        true
    }

    /// Find a free area with `len`, at or above `min_offset` (the caller's
    /// floor — see `determine_offset`'s `MMAP_MIN_ADDR`; pass 0 for no floor).
    ///
    /// The old implementation tried every mapping's end address as a candidate
    /// start and `test_map`'d each — O(n) candidates times an O(n) overlap scan,
    /// the inner half of the `mmap` O(n²). This keeps the two behaviours that
    /// matter (never below the floor, reuse holes left by `munmap`) but reaches
    /// the common case in O(log n):
    ///
    /// * Fast path — because mappings never overlap, the entry with the greatest
    ///   start also has the greatest end, so the slot just past it is free of
    ///   every mapping. That is where an upward-growing workload (Mesa maps
    ///   thousands of anonymous regions) belongs, and the tree hands back its
    ///   last entry in O(log n) with no walk of the packed low space. Only a
    ///   child sub-VMAR can sit above the top mapping, so that is all this has to
    ///   exclude.
    /// * Fallback — the top is full or a child blocks it: first-fit from the
    ///   floor, jumping the cursor past each occupied range (a straddling
    ///   mapping, a blocking child, or the next mapping starting inside the
    ///   window) in address order until a `len`-sized hole opens.
    fn find_free_area(
        &self,
        inner: &VmarInner,
        min_offset: usize,
        len: usize,
        align: usize,
    ) -> Option<usize> {
        // TODO: randomize
        debug_assert!(check_aligned(min_offset, align));
        debug_assert!(check_aligned(len, align));
        let base = self.addr;
        let limit = base + self.size;
        let floor = base + min_offset;

        // Fast path: just above the highest mapping (or the floor, if higher).
        let top = inner
            .mappings
            .values()
            .next_back()
            .map(|m| m.end_addr())
            .unwrap_or(base);
        let cand = floor.max(top).next_multiple_of(align);
        if cand.checked_add(len).is_some_and(|e| e <= limit)
            && !inner
                .children
                .iter()
                .any(|ch| ch.addr() < cand + len && ch.end_addr() > cand)
        {
            debug_assert!(self.test_map(inner, cand - base, len, align));
            return Some(cand - base);
        }

        // Fallback: first-fit from the floor. Each iteration advances the cursor
        // strictly past one occupied range and re-probes, so it terminates.
        let mut cursor = floor;
        loop {
            match cursor.checked_add(len) {
                Some(e) if e <= limit => {}
                _ => return None,
            }
            // A mapping straddling the cursor (starts at/before it, ends after).
            if let Some((_, m)) = inner.mappings.range(..=cursor).next_back() {
                if m.end_addr() > cursor {
                    cursor = m.end_addr().next_multiple_of(align);
                    continue;
                }
            }
            // A child overlapping [cursor, cursor+len): jump past the farthest.
            if let Some(end) = inner
                .children
                .iter()
                .filter(|ch| ch.addr() < cursor + len && ch.end_addr() > cursor)
                .map(|ch| ch.end_addr())
                .max()
            {
                cursor = end.next_multiple_of(align);
                continue;
            }
            // A mapping starting inside the window: the hole is too small.
            if let Some((_, m)) = inner.mappings.range(cursor..cursor + len).next() {
                cursor = m.end_addr().next_multiple_of(align);
                continue;
            }
            // Clear of mappings and children.
            debug_assert!(self.test_map(inner, cursor - base, len, align));
            return Some(cursor - base);
        }
    }

    /// Get the end address (base + size) of this VMAR.
    pub fn end_addr(&self) -> VirtAddr {
        self.addr + self.size
    }

    fn overlap(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        !(self.addr >= end || self.end_addr() <= begin)
    }

    fn within(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        begin <= self.addr && self.end_addr() <= end
    }

    fn partial_overlap(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        self.overlap(begin, end) && !self.within(begin, end)
    }

    /// return true if vmar contains vaddr, or return false.
    pub fn contains(&self, vaddr: VirtAddr) -> bool {
        self.addr <= vaddr && vaddr < self.end_addr()
    }

    /// Get information of this VmAddressRegion
    pub fn get_info(&self) -> VmarInfo {
        // pub fn get_info(&self, va: usize) -> VmarInfo {
        // let _r = self.page_table.lock().query(va);
        VmarInfo {
            base: self.addr(),
            len: self.size,
            // pg_token: self.page_table.lock().table_phys(),
        }
    }

    /// Get VmarFlags of this VMAR.
    pub fn get_flags(&self) -> VmarFlags {
        self.flags
    }

    /// Get base address of vdso.
    pub fn vdso_base_addr(&self) -> Option<usize> {
        let guard = self.inner.lock();
        let inner = guard.as_ref().unwrap();
        for map in inner.mappings.values() {
            if map.vmo.name().starts_with("vdso") && map.inner.lock().vmo_offset == 0x7000 {
                return Some(map.addr());
            }
        }
        for vmar in inner.children.iter() {
            if let Some(addr) = vmar.vdso_base_addr() {
                return Some(addr);
            }
        }
        None
    }

    /// Handle page fault happened on this VMAR.
    ///
    /// The fault virtual address is `vaddr` and the reason is in `flags`.
    pub fn handle_page_fault(&self, vaddr: VirtAddr, flags: MMUFlags) -> ZxResult {
        // Resolve the owning child sub-VMAR / mapping UNDER the lock, then
        // release the lock BEFORE doing the actual fault work.
        //
        // `self.inner` is an IRQ-disabling spinlock. `mapping.handle_page_fault`
        // demand-pages from disk (commit_page -> FrameFiller -> block device),
        // and with fault-around it commits up to 16 pages per fault. Holding
        // this lock across all of that keeps interrupts OFF for the entire
        // multi-page (polled) disk-read window — starving the timer, scheduler
        // and input for as long as the paging takes. On a real polled-AHCI disk
        // a compositor's clients (swaybg/foot) paging in tens of MiB of
        // libraries then presents as a full-machine hang. Dropping the lock
        // first bounds the interrupts-off window to a single page read, with
        // the CPU able to service interrupts between reads.
        //
        // The child/mapping are reference-counted (`Arc`); cloning them keeps
        // them alive if a concurrent VMAR edit removes them from the list, and
        // the actual page-table update is serialized by the mapping's own
        // page-table lock, so releasing the VMAR lock here is safe.
        //
        // Prefer a child sub-VMAR that owns this address — but if the child can
        // NOT resolve the fault (no mapping there), fall through to this VMAR's
        // own mappings instead of returning the child's NOT_FOUND. A mapping
        // placed in the parent must not be shadowed into a spurious SIGSEGV just
        // because some child sub-region's address *range* happens to cover
        // `vaddr` while owning no mapping for it.
        let (child, mapping) = {
            let guard = self.inner.lock();
            let inner = guard.as_ref().unwrap();
            if !self.contains(vaddr) {
                return Err(ZxError::NOT_FOUND);
            }
            let child = inner.children.iter().find(|ch| ch.contains(vaddr)).cloned();
            // O(log n) point lookup. The old linear scan grew a move-to-front
            // hit cache to stay fast under a desktop's hundreds of mappings; the
            // tree makes the repeat fault O(log n) with no cache and no mutation.
            let mapping = Self::mapping_containing(inner, vaddr);
            (child, mapping)
        };
        if let Some(child) = child {
            match child.handle_page_fault(vaddr, flags) {
                Err(ZxError::NOT_FOUND) => {}
                res => return res,
            }
        }
        if let Some(mapping) = mapping {
            return mapping.handle_page_fault(vaddr, flags);
        }
        Err(ZxError::NOT_FOUND)
    }

    /// Log every mapping whose range comes within `window` bytes of `vaddr`.
    ///
    /// Diagnostic for the open "malloc returned unmapped memory" bug: a
    /// user fault answered NOT_FOUND proves no VmMapping covers the address,
    /// but not WHY. The neighbours discriminate the two candidates outright:
    ///   * a mapping that ENDS just below `vaddr` (or starts just above)
    ///     means a region was created too short, or was cut back;
    ///   * no mapping anywhere near means the address was never mapped at
    ///     all, i.e. mmap handed back a range it did not install.
    pub fn dump_near(&self, vaddr: VirtAddr, window: usize) {
        // Integrity sweep first. Several kernel objects have been found with
        // their type identity overwritten (a Process ext that will not
        // downcast; an Arc<Thread> in Process::threads whose downcast_arc
        // unwrap panics), so the VmMapping list is a prime candidate for the
        // same damage -- and a corrupted addr/size is exactly how a live
        // region would stop covering an address userspace still holds. Report
        // any mapping that breaks an invariant it cannot legally break.
        let mut bad = 0usize;
        let mut total = 0usize;
        self.for_each_mapping(&mut |map| {
            let inner = map.inner.lock();
            total += 1;
            let unaligned = !page_aligned(inner.addr) || !page_aligned(inner.size);
            let empty = inner.size == 0;
            // User mappings must live below the kernel half.
            let kernel_range = inner.addr as u64 >= KERNEL_ASPACE_BASE;
            let overflows = inner.addr.checked_add(inner.size).is_none();
            // flags is per page: its length must match the size.
            let flags_mismatch = !empty && inner.flags.len() != pages(inner.size);
            if unaligned || empty || kernel_range || overflows || flags_mismatch {
                bad += 1;
                error!(
                    "[vmar-corrupt] mapping [{:#x}, +{:#x}) vmo_offset={:#x} flags_len={} \
                     pages={} -- unaligned={} empty={} kernel_range={} overflows={} \
                     flags_mismatch={}",
                    inner.addr,
                    inner.size,
                    inner.vmo_offset,
                    inner.flags.len(),
                    pages(inner.size),
                    unaligned,
                    empty,
                    kernel_range,
                    overflows,
                    flags_mismatch,
                );
            }
        });
        if bad > 0 {
            error!(
                "[vmar-corrupt] {} of {} mappings are structurally invalid -- the VMAR \
                 itself is damaged, not just this address",
                bad, total
            );
        }

        // Who removed this address? Neither [mmap-lost] nor [vmar-corrupt]
        // fires on the crashing boot, so the range WAS installed correctly and
        // the mappings are sound -- something unmapped it afterwards. Print any
        // recorded unmap that covered the faulting address, newest last.
        {
            let guard = self.inner.lock();
            if let Some(inner) = guard.as_ref() {
                let mut hit = false;
                let mut used = 0usize;
                for (a, l, why) in inner.recent_unmaps.iter() {
                    // Unwritten slots are the (0, 0, "") default; skip them.
                    if *l == 0 {
                        continue;
                    }
                    used += 1;
                    if vaddr >= *a && vaddr < a + l {
                        error!(
                            "[vmar-unmapped-by] {:#x}: removed by {} of [{:#x}, {:#x})",
                            vaddr,
                            why,
                            a,
                            a + l
                        );
                        hit = true;
                    }
                }
                if !hit && used > 0 {
                    let prev =
                        inner.recent_unmaps[(inner.unmap_head + UNMAP_HISTORY - 1) % UNMAP_HISTORY];
                    error!(
                        "[vmar-unmapped-by] {:#x}: NOT covered by any of the last {} unmaps \
                         (most recent: [{:#x}, +{:#x}) by {})",
                        vaddr, used, prev.0, prev.1, prev.2,
                    );
                }
            }
        }

        let lo = vaddr.saturating_sub(window);
        let hi = vaddr.saturating_add(window);
        let mut n = 0usize;
        self.for_each_mapping(&mut |map| {
            let inner = map.inner.lock();
            if inner.end_addr() > lo && inner.addr < hi {
                error!(
                    "[vmar-near] {:#x}: mapping [{:#x}, {:#x}) size={:#x} vmo_offset={:#x} vmo_len={:#x}",
                    vaddr,
                    inner.addr,
                    inner.end_addr(),
                    inner.size,
                    inner.vmo_offset,
                    map.vmo.len(),
                );
                n += 1;
            }
        });
        if n == 0 {
            error!(
                "[vmar-near] {:#x}: NO mapping within {:#x} bytes -- the address was never mapped",
                vaddr, window
            );
        }
    }

    fn for_each_mapping(&self, f: &mut impl FnMut(&Arc<VmMapping>)) {
        let guard = self.inner.lock();
        let inner = guard.as_ref().unwrap();
        for map in inner.mappings.values() {
            f(map);
        }
        for child in inner.children.iter() {
            child.for_each_mapping(f);
        }
    }

    /// Clone the entire address space and VMOs from source VMAR. (For Linux fork)
    /// Fork `src`'s address space into this one.
    ///
    /// `gather_shootdowns` batches the cross-CPU TLB shootdowns of the whole
    /// fork into one. Copy-on-write write-protects every mapping of the parent,
    /// and each mapping costs *two* full shootdowns — one inside
    /// `VmObject::create_child`, one in `VmMapping::protect_for_cow` — each an
    /// IPI round trip to every other CPU with an ack spin-wait. That fixed
    /// per-mapping price is what made COW `fork` of a *small* process 3x more
    /// expensive than the eager copy it replaced, while still being far cheaper
    /// for a large one: the saving is per page, the cost is per mapping.
    ///
    /// It is the caller's job to pass `true` only when no other thread can be
    /// executing in `src`'s address space. Between write-protecting a page and
    /// flushing, another CPU can write through a stale writable TLB entry onto a
    /// frame the child already shares; ungathered that window is a few
    /// instructions, gathered it is the whole fork. A single-threaded parent has
    /// no such CPU — its one thread is the one blocked here, and every path that
    /// stops running a thread loads another page-table root, which on x86_64
    /// without PCID flushes every non-global entry.
    pub fn fork_from(&self, src: &Arc<Self>, gather_shootdowns: bool) -> ZxResult {
        // Snapshot the source tree's mappings under its lock, then do the
        // actual copy with NO VMAR lock held.
        //
        // `clone_map` eagerly copies every page of every mapping; for
        // demand-paged file mappings each page can be a (polled) disk read.
        // The VMAR `inner` locks are IRQ-disabling spinlocks — the previous
        // code held BOTH this VMAR's and the source's lock across the entire
        // walk, keeping interrupts off for the whole multi-second copy of a
        // large process (a Wayland compositor maps 100+ MiB of libraries).
        // The machine appeared dead the moment the compositor forked its
        // autostart shell. A point-in-time snapshot is sound: the forking
        // parent is blocked inside the fork syscall, and the mapping Arcs
        // keep the objects alive regardless of concurrent VMAR edits.
        let mut src_mappings = Vec::new();
        Self::collect_mappings_for_fork(src, &mut src_mappings);

        // Copy each mapping with no VMAR lock held. Physical VMOs are Arc-shared
        // (instant); paged VMOs copy-on-write. `map_committed` installs PTEs only
        // for already-committed pages, leaving the rest to demand-fault — so a
        // large compositor fork no longer stalls eagerly populating page tables.
        //
        // The gather window is opened on the PARENT's page table, which is where
        // the write-protection happens; the child's is untouched by it (and
        // needs no shootdown at all — no CPU has ever had it loaded).
        let gather_shootdowns = gather_shootdowns && FORK_GATHER.load(Ordering::Relaxed);
        if gather_shootdowns {
            src.page_table.lock().set_gather(true);
        }
        let mut new_mappings = Vec::with_capacity(src_mappings.len());
        let mut result = Ok(());
        // COW-child dedup, keyed by source VMO koid. An mprotect or a
        // hole-punching munmap splits a mapping while keeping the SAME VMO Arc
        // (see `cut`: the tail is `vmo: self.vmo.clone()`), so a process can
        // have many mappings over one VMO. Cloning each independently calls
        // `create_child` once per mapping, and `create_child` write-protects
        // EVERY mapper of the VMO (paged.rs:1343) — so M mappings over one VMO
        // cost O(M²) and stack M redundant hidden nodes. Snapshot each unique
        // VMO once and map every sibling into that single child: the first
        // `create_child` already write-protected all the siblings' PTEs in its
        // mapper walk, so the rest need only a fresh `VmMapping` into the shared
        // child. Aliased siblings (same offset) now correctly share their COW
        // child too, instead of diverging into separate snapshots.
        let mut cow_cache: BTreeMap<u64, Arc<VmObject>> = BTreeMap::new();
        for map in src_mappings.into_iter() {
            match map
                .clone_map(self.page_table.clone(), &mut cow_cache)
                .and_then(|mapping| {
                    let t0 = kernel_hal::timer::timer_now().as_nanos() as u64;
                    let r = mapping.map_committed();
                    let t1 = kernel_hal::timer::timer_now().as_nanos() as u64;
                    note_map_committed(t1 - t0);
                    r.map(|_| mapping)
                }) {
                Ok(mapping) => new_mappings.push(mapping),
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        // Close the window and pay the debt exactly once — on the error path
        // too. Leaving a window open would silently suppress every later
        // shootdown on this address space, and leaving the parent with stale
        // writable entries onto frames the partially-built child already shares
        // is the corruption this whole dance exists to prevent.
        if gather_shootdowns {
            let mut pg_table = src.page_table.lock();
            if pg_table.set_gather(false) {
                pg_table.remote_flush_all();
            }
        }
        result?;

        let mut guard = self.inner.lock();
        let inner = guard.as_mut().ok_or(ZxError::BAD_STATE)?;
        // The snapshot flattened the source's whole tree (parent + children);
        // every mapping in it is pairwise non-overlapping, so their start
        // addresses are distinct keys.
        for mapping in new_mappings {
            inner.mappings.insert(mapping.addr(), mapping);
        }
        Ok(())
    }

    /// Collect (a snapshot of) every mapping in `vmar`'s tree for `fork_from`,
    /// children first — the same flattening order the fork used when it walked
    /// the tree directly.
    fn collect_mappings_for_fork(vmar: &Arc<Self>, out: &mut Vec<Arc<VmMapping>>) {
        let guard = vmar.inner.lock();
        if let Some(inner) = guard.as_ref() {
            for child in inner.children.iter() {
                Self::collect_mappings_for_fork(child, out);
            }
            out.extend(inner.mappings.values().cloned());
        }
    }

    /// Dump VMAR tree (for debugging).
    pub fn dump(&self) {
        // The walk takes every node's spinlock even though its only output is
        // `info!` lines; skip it entirely when nothing would be emitted (the
        // vfork path calls this twice per vfork on a many-hundred-mapping
        // address space).
        if log::log_enabled!(log::Level::Info) {
            self.dump_impl(0);
        }
    }

    fn dump_impl(&self, depth: usize) {
        let guard = self.inner.lock();
        if let Some(inner) = guard.as_ref() {
            info!(
                "VMAR depth {}: {:#x} - {:#x} (size={:#x})",
                depth,
                self.addr,
                self.addr + self.size,
                self.size
            );
            for map in inner.mappings.values() {
                info!(
                    "  Map depth {}: {:#x} - {:#x} (size={:#x}) VMO: {}",
                    depth,
                    map.addr(),
                    map.addr() + map.size(),
                    map.size(),
                    map.vmo.name()
                );
            }
            for child in inner.children.iter() {
                child.dump_impl(depth + 1);
            }
        }
    }

    /// Returns statistics about memory used by a task.
    pub fn get_task_stats(&self) -> TaskStatsInfo {
        let mut task_stats = TaskStatsInfo::default();
        self.for_each_mapping(&mut |map| map.fill_in_task_status(&mut task_stats));
        task_stats
    }

    /// Read from address space.
    ///
    /// Return the actual number of bytes read.
    pub fn read_memory(&self, vaddr: usize, buf: &mut [u8]) -> ZxResult<usize> {
        // TODO: support multiple VMOs
        let map = self.find_mapping(vaddr).ok_or(ZxError::NO_MEMORY)?;
        let map_inner = map.inner.lock();
        let vmo_offset = vaddr - map_inner.addr + map_inner.vmo_offset;
        let size_limit = map_inner.addr + map_inner.size - vaddr;
        let actual_size = buf.len().min(size_limit);
        map.vmo.read(vmo_offset, &mut buf[0..actual_size])?;
        Ok(actual_size)
    }

    /// Write to address space.
    ///
    /// Return the actual number of bytes written.
    pub fn write_memory(&self, vaddr: usize, buf: &[u8]) -> ZxResult<usize> {
        // TODO: support multiple VMOs
        let map = self.find_mapping(vaddr).ok_or(ZxError::NO_MEMORY)?;
        let map_inner = map.inner.lock();
        let vmo_offset = vaddr - map_inner.addr + map_inner.vmo_offset;
        let size_limit = map_inner.addr + map_inner.size - vaddr;
        let actual_size = buf.len().min(size_limit);
        map.vmo.write(vmo_offset, &buf[0..actual_size])?;
        Ok(actual_size)
    }

    /// Find mapping of vaddr
    pub fn find_mapping(&self, vaddr: usize) -> Option<Arc<VmMapping>> {
        let guard = self.inner.lock();
        let inner = guard.as_ref().unwrap();
        // O(log n) point lookup (replaces the linear scan + move-to-front hit
        // cache that `read_memory` / `write_memory` relied on).
        if let Some(mapping) = Self::mapping_containing(inner, vaddr) {
            return Some(mapping);
        }
        if let Some(child) = inner.children.iter().find(|ch| ch.contains(vaddr)) {
            return child.find_mapping(vaddr);
        }
        None
    }

    /// Describe the mapping that contains `vaddr`, for a crash dump: the
    /// mapping's start address, the byte offset of `vaddr` within its backing
    /// file (VMO), and the backing object's name (a file path for a file
    /// mapping, empty for anonymous memory).
    ///
    /// This is what turns a bare userspace fault `pc=0x7f…` into
    /// `libwlroots.so.13 + 0x1234`, so a dynamically-linked crash (labwc and
    /// its libraries) can be symbolised offline with `addr2line`/`objdump`
    /// against the on-disk library, instead of leaving only a `Done(139)`.
    pub fn describe_addr(&self, vaddr: usize) -> Option<(VirtAddr, usize, alloc::string::String)> {
        let map = self.find_mapping(vaddr)?;
        let inner = map.inner.lock();
        // File offset of `vaddr`: the VMO offset the mapping starts at plus how
        // far `vaddr` is into the mapping.
        let file_off = inner.vmo_offset + vaddr.saturating_sub(inner.addr);
        Some((inner.addr, file_off, map.vmo.name()))
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().unwrap();
        inner.mappings.len() + inner.children.len()
    }

    #[cfg(test)]
    fn used_size(&self) -> usize {
        let mut guard = self.inner.lock();
        let inner = guard.as_mut().unwrap();
        let map_size: usize = inner.mappings.values().map(|map| map.size()).sum();
        let vmar_size: usize = inner.children.iter().map(|vmar| vmar.size).sum();
        map_size + vmar_size
    }
}

/// Information of a VmAddressRegion.
#[repr(C)]
#[derive(Debug)]
pub struct VmarInfo {
    base: usize,
    len: usize,
    // pg_token: usize,
}

/// One row of a `/proc/<pid>/maps`-style mapping listing
/// (see [`VmAddressRegion::mappings_dump`]).
#[derive(Debug, Clone)]
pub struct MappingDump {
    /// First mapped address.
    pub start: VirtAddr,
    /// One past the last mapped address.
    pub end: VirtAddr,
    /// MMU flags of the mapping's first page.
    pub flags: MMUFlags,
    /// Byte offset into the backing VMO.
    pub vmo_offset: usize,
    /// Whether the backing VMO is shared with other mappers (MAP_SHARED-like).
    pub shared: bool,
    /// Koid of the backing VMO — stands in for the inode column.
    pub vmo_id: u64,
    /// Kernel-object name of the backing VMO; empty for plain anonymous memory.
    pub name: alloc::string::String,
}

/// Virtual Memory Mapping
pub struct VmMapping {
    /// The permission limitation of the vmar
    permissions: MMUFlags,
    vmo: Arc<VmObject>,
    page_table: Arc<Mutex<dyn GenericPageTable>>,
    inner: Mutex<VmMappingInner>,
}

#[derive(Debug, Clone)]
struct VmMappingInner {
    /// The actual flags used in the mapping of each page
    flags: Vec<MMUFlags>,
    addr: VirtAddr,
    size: usize,
    vmo_offset: usize,
}

/// Statistics about resources (e.g., memory) used by a task.
#[repr(C)]
#[derive(Default)]
pub struct TaskStatsInfo {
    mapped_bytes: u64,
    private_bytes: u64,
    shared_bytes: u64,
    scaled_shared_bytes: u64,
}

impl TaskStatsInfo {
    /// Total mapped bytes — the `VmSize` of `/proc/<pid>/status`.
    pub fn mapped_bytes(&self) -> u64 {
        self.mapped_bytes
    }

    /// Committed bytes private to this task.
    pub fn private_bytes(&self) -> u64 {
        self.private_bytes
    }

    /// Committed bytes shared with other mappers.
    pub fn shared_bytes(&self) -> u64 {
        self.shared_bytes
    }
}

impl core::fmt::Debug for VmMapping {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("VmMapping")
            .field("addr", &inner.addr)
            .field("size", &inner.size)
            .field("permissions", &self.permissions)
            // .field("flags", &inner.flags)
            .field("vmo_id", &self.vmo.id())
            .field("vmo_offset", &inner.vmo_offset)
            .finish()
    }
}

impl VmMapping {
    fn new(
        addr: VirtAddr,
        size: usize,
        vmo: Arc<VmObject>,
        vmo_offset: usize,
        permissions: MMUFlags,
        flags: MMUFlags,
        page_table: Arc<Mutex<dyn GenericPageTable>>,
    ) -> Arc<Self> {
        let mapping = Arc::new(VmMapping {
            inner: Mutex::new(VmMappingInner {
                flags: vec![flags; pages(size)],
                addr,
                size,
                vmo_offset,
            }),
            permissions,
            page_table,
            vmo: vmo.clone(),
        });
        info!(
            "VmMapping::new: addr={:#x}, size={:#x}, vmo_id={}",
            addr,
            size,
            vmo.id()
        );
        vmo.append_mapping(Arc::downgrade(&mapping));
        mapping
    }

    /// Map range and commit.
    /// Commit pages to vmo, and map those to frames in page_table.
    /// Temporarily used for development. A standard procedure for
    /// vmo is: create_vmo, op_range(commit), map
    /// Lazy map for fork: install PTEs ONLY for pages the VMO already has
    /// committed; untouched pages stay unmapped and demand-fault in the child
    /// exactly like they would have in the parent (zero-fill or backing-source
    /// fill on first touch).
    ///
    /// This replaces `map()` on the fork path. `map()` COMMITS every page of
    /// the vmo up front, which on hardware turned a 107 MiB mapping the parent
    /// had barely touched (copy=0 — nothing committed to duplicate) into 6.4s
    /// of per-page content creation (~240us/page: disk fill / zero-fill +
    /// allocation) for pages the child may never read. Multiplied across
    /// labwc's 596-mapping address space and two client spawns, that was the
    /// minutes-long "frozen" fork. Physical windows (framebuffer/dumb buffers)
    /// report every page committed, so they still map fully and eagerly.
    fn map_committed(self: &Arc<Self>) -> ZxResult {
        let inner = self.inner.lock();
        let mut page_table = self.page_table.lock();
        let page_num = inner.size / PAGE_SIZE;
        let vmo_offset = inner.vmo_offset / PAGE_SIZE;
        for i in 0..page_num {
            if let Some(paddr) = self.vmo.committed_paddr(vmo_offset + i) {
                page_table
                    .map(
                        Page::new_aligned(inner.addr + i * PAGE_SIZE, PageSize::Size4K),
                        paddr,
                        inner.flags[i],
                    )
                    .expect("failed to map");
            }
        }
        Ok(())
    }

    fn map(self: &Arc<Self>) -> ZxResult {
        self.vmo.commit_pages_with(&mut |commit| {
            let inner = self.inner.lock();
            let mut page_table = self.page_table.lock();
            let page_num = inner.size / PAGE_SIZE;
            let vmo_offset = inner.vmo_offset / PAGE_SIZE;
            for i in 0..page_num {
                let paddr = commit(vmo_offset + i, inner.flags[i])?;
                page_table
                    .map(
                        Page::new_aligned(inner.addr + i * PAGE_SIZE, PageSize::Size4K),
                        paddr,
                        inner.flags[i],
                    )
                    .expect("failed to map");
            }
            Ok(())
        })
    }

    fn unmap(&self) {
        let inner = self.inner.lock();
        // TODO inner.vmo_offset unused?
        self.page_table
            .lock()
            .unmap_cont(inner.addr, inner.size)
            .expect("failed to unmap")
    }

    fn fill_in_task_status(&self, task_stats: &mut TaskStatsInfo) {
        let (start_idx, end_idx) = {
            let inner = self.inner.lock();
            let start_idx = inner.vmo_offset / PAGE_SIZE;
            (start_idx, start_idx + inner.size / PAGE_SIZE)
        };
        task_stats.mapped_bytes += self.vmo.len() as u64;
        let committed_pages = self.vmo.committed_pages_in_range(start_idx, end_idx);
        let share_count = self.vmo.share_count();
        if share_count == 1 {
            task_stats.private_bytes += (committed_pages * PAGE_SIZE) as u64;
        } else {
            task_stats.shared_bytes += (committed_pages * PAGE_SIZE) as u64;
            task_stats.scaled_shared_bytes += (committed_pages * PAGE_SIZE / share_count) as u64;
        }
    }

    /// Cut and unmap regions in `[begin, end)`.
    ///
    /// If it will be split, return another one.
    fn cut(&self, begin: VirtAddr, end: VirtAddr) -> Option<Arc<Self>> {
        if !self.overlap(begin, end) {
            return None;
        }
        let mut inner = self.inner.lock();
        let mut page_table = self.page_table.lock();
        if inner.addr >= begin && inner.end_addr() <= end {
            // subset: [xxxxxxxxxx]
            page_table
                .unmap_cont(inner.addr, inner.size)
                .expect("failed to unmap");
            inner.size = 0;
            inner.flags.clear();
            None
        } else if inner.addr >= begin && inner.addr < end {
            // prefix: [xxxx------]
            let cut_len = end - inner.addr;
            page_table
                .unmap_cont(inner.addr, cut_len)
                .expect("failed to unmap");
            inner.addr = end;
            inner.size -= cut_len;
            inner.vmo_offset += cut_len;
            inner.flags.drain(0..pages(cut_len));
            None
        } else if inner.end_addr() <= end && inner.end_addr() > begin {
            // postfix: [------xxxx]
            let cut_len = inner.end_addr() - begin;
            let new_len = begin - inner.addr;
            page_table
                .unmap_cont(begin, cut_len)
                .expect("failed to unmap");
            inner.size = new_len;
            inner.flags.truncate(pages(new_len));
            None
        } else {
            // superset: [---xxxx---]
            let cut_len = end - begin;
            let new_len1 = begin - inner.addr;
            let new_len2 = inner.end_addr() - end;
            page_table
                .unmap_cont(begin, cut_len)
                .expect("failed to unmap");
            let new_flags_range = (pages(inner.size) - pages(new_len2))..pages(inner.size);
            let new_mapping = Arc::new(VmMapping {
                permissions: self.permissions,
                vmo: self.vmo.clone(),
                page_table: self.page_table.clone(),
                inner: Mutex::new(VmMappingInner {
                    flags: inner.flags.drain(new_flags_range).collect(),
                    addr: end,
                    size: new_len2,
                    vmo_offset: inner.vmo_offset + (end - inner.addr),
                }),
            });
            inner.size = new_len1;
            inner.flags.truncate(pages(new_len1));
            Some(new_mapping)
        }
    }

    fn overlap(&self, begin: VirtAddr, end: VirtAddr) -> bool {
        let inner = self.inner.lock();
        !(inner.addr >= end || inner.end_addr() <= begin)
    }

    fn contains(&self, vaddr: VirtAddr) -> bool {
        let inner = self.inner.lock();
        inner.addr <= vaddr && vaddr < inner.end_addr()
    }

    fn is_valid_mapping_flags(&self, flags: MMUFlags) -> bool {
        self.permissions.contains(flags & MMUFlags::RXW)
    }

    fn protect(&self, flags: MMUFlags, start_index: usize, end_index: usize) {
        let mut inner = self.inner.lock();
        let mut pg_table = self.page_table.lock();
        // mmu-gather: defer the cross-CPU shootdown to one flush after the
        // loop; a synchronous shootdown per page livelocks large mprotects
        // whenever a peer CPU can't ack promptly.
        for i in start_index..end_index {
            let mut new_flags = inner.flags[i];
            new_flags.remove(MMUFlags::RXW);
            new_flags.insert(flags & MMUFlags::RXW);
            inner.flags[i] = new_flags;
            let va = inner.addr + i * PAGE_SIZE;
            // A frame this VMO does not OWN at this index must never be made
            // writable in place. Two kinds of PTE point at such frames:
            //
            // * the global ZERO_FRAME (a read-fault on demand-zero memory maps
            //   it, and no per-page frame is tracked). Raising WRITE in place
            //   let a store land in the shared zero page with no copy-on-write,
            //   poisoning every future demand-zero fault — the source of
            //   mimalloc's "corrupted free list entry" abort;
            //
            // * a page-cache frame borrowed by a MAP_PRIVATE file mapping (the
            //   read fault maps the cache VMO's own frame read-only). Raising
            //   WRITE in place would let this process scribble on the shared
            //   page cache — every other process mapping the library would read
            //   the corruption. This is the ld.so pattern (`mprotect` text to
            //   RW for DT_TEXTREL/GNU_RELRO), so it is hit in practice.
            //
            // Both are the same rule: if the VMO's committed frame for this
            // index is not the frame in the PTE, drop the PTE instead of
            // updating it. The next write re-faults into `commit_page(WRITE)`,
            // which performs the copy the read-only PTE existed to force.
            // Never-faulted leaves are left untouched (`query` -> NotMapped),
            // so a huge PROT_NONE reservation stays cheap.
            let vmo_page_idx = (inner.vmo_offset + i * PAGE_SIZE) / PAGE_SIZE;
            let unowned_frame = match pg_table.query(va) {
                Ok((paddr, _, _)) => self.vmo.committed_paddr(vmo_page_idx) != Some(paddr),
                Err(_) => false,
            };
            if unowned_frame && new_flags.contains(MMUFlags::WRITE) {
                pg_table.unmap_no_shootdown(va).ignore().unwrap();
            } else {
                pg_table
                    .update_no_shootdown(va, None, Some(new_flags))
                    .ignore()
                    .unwrap();
            }
        }
        if start_index < end_index {
            pg_table.remote_flush_all();
        }
    }

    /// `madvise(MADV_DONTNEED)` for the sub-range `[begin, end)` of this mapping:
    /// make every committed page read back as zero.
    ///
    /// We zero the frame IN PLACE through the VMO's `committed_paddr` (a physmap
    /// write), NOT by walking the current page table. That is the whole point:
    /// a page whose frame the VMO still owns but whose PTE was dropped or turned
    /// PROT_NONE is invisible to a page-table walk, so the earlier PTE-based
    /// zeroing skipped it — and mimalloc, reusing exactly such a page, found
    /// stale bytes where it assumes fresh OS zeros and aborted ("corrupted free
    /// list entry"). Zeroing in place also leaves the mapping and its frame
    /// intact, so a subsequent access neither faults through a now-stale
    /// per-page flag nor races a freed frame (the unmap+decommit variant did,
    /// turning the abort into a SIGSEGV).
    fn dontneed(&self, begin: VirtAddr, end: VirtAddr) {
        // Snapshot geometry under a brief lock; do NOT hold the mapping lock
        // across the VMO calls below (the borrow/mutex discipline forbids the
        // mapping->VMO lock nesting that would otherwise deadlock a concurrent
        // range_change).
        let (map_addr, vmo_offset_pages) = {
            let inner = self.inner.lock();
            (inner.addr, inner.vmo_offset / PAGE_SIZE)
        };
        // Never zero a shared object (MAP_SHARED / wl_shm): that is another
        // process's live data, not this caller's private scratch.
        if self.vmo.share_count() > 1 {
            return;
        }
        let mut va = round_down_pages(begin.max(map_addr));
        let end = round_down_pages(end);
        while va < end {
            let vmo_page = vmo_offset_pages + (va - map_addr) / PAGE_SIZE;
            // (1) Zero the frame the VMO currently owns for this page. This
            //     reaches a committed page whose PTE was dropped / turned
            //     PROT_NONE, which a page-table walk cannot see.
            if let Some(pa) = self.vmo.committed_paddr(vmo_page) {
                kernel_hal::mem::pmem_zero(pa, PAGE_SIZE);
            }
            // (2) Drop the PTE — but NOT the frame. The abort actually came from
            //     a STALE TRANSLATION: the page table still maps this VA to a
            //     frame the VMO no longer tracks (committed_paddr == None), so
            //     zeroing through the VMO missed it and the old bytes kept being
            //     served. Unmapping forces the next touch to re-resolve through
            //     the VMO — a fresh demand-zero page, or the frame just zeroed in
            //     (1). We deliberately do NOT free any frame (the decommit
            //     variant did, and freeing one the allocator still used turned
            //     the abort into a SIGSEGV).
            {
                let mut pg = self.page_table.lock();
                let _ = pg.unmap(va);
            }
            va += PAGE_SIZE;
        }
    }

    fn size(&self) -> usize {
        self.inner.lock().size
    }

    fn addr(&self) -> VirtAddr {
        self.inner.lock().addr
    }

    fn end_addr(&self) -> VirtAddr {
        self.inner.lock().end_addr()
    }

    /// Get MMUFlags of this VmMapping.
    pub fn get_flags(&self, vaddr: usize) -> ZxResult<MMUFlags> {
        if self.contains(vaddr) {
            let page_id = (vaddr - self.addr()) / PAGE_SIZE;
            Ok(self.inner.lock().flags[page_id])
        } else {
            Err(ZxError::NO_MEMORY)
        }
    }

    /// query vaddr's PhysAddr, PhysAddr, PageSize.
    pub fn query_vaddr(&self, vaddr: usize) -> PagingResult<(PhysAddr, MMUFlags, PageSize)> {
        self.page_table.lock().query(vaddr)
    }

    /// Write `buf` through this mapping if `vaddr..vaddr+buf.len()` lies fully
    /// inside it; returns `Ok(false)` (without writing) when it does not, so a
    /// caller holding a cached mapping can fall back to a fresh lookup. Lets
    /// hot writers (ELF relocation applies thousands of 8-byte writes to the
    /// same GOT/PLT mapping) skip the per-write VMAR-tree scan.
    pub fn write_memory_if_contains(&self, vaddr: usize, buf: &[u8]) -> ZxResult<bool> {
        let vmo_offset = {
            let inner = self.inner.lock();
            if vaddr < inner.addr || vaddr + buf.len() > inner.addr + inner.size {
                return Ok(false);
            }
            vaddr - inner.addr + inner.vmo_offset
        };
        self.vmo.write(vmo_offset, buf)?;
        Ok(true)
    }

    /// Remove WRITE flag from the mappings for Copy-on-Write.
    pub(super) fn range_change(&self, offset: usize, len: usize, op: RangeChangeOp) {
        let inner = self.inner.try_lock();
        // If we are already locked, we are handling page fault/map range
        // In this case we can just ignore the operation since we will update the mapping later
        if let Some(inner) = inner {
            let start = offset.max(inner.vmo_offset);
            let end = (inner.vmo_offset + inner.size / PAGE_SIZE).min(offset + len);
            if !(start..end).is_empty() {
                let mut pg_table = self.page_table.lock();
                // mmu-gather: one cross-CPU shootdown for the whole range
                // (see `protect` above); per-page synchronous shootdowns
                // livelock when a peer CPU can't ack.
                for i in (start - inner.vmo_offset)..(end - inner.vmo_offset) {
                    match op {
                        RangeChangeOp::RemoveWrite => {
                            let mut new_flag = inner.flags[i];
                            new_flag.remove(MMUFlags::WRITE);
                            pg_table
                                .update_no_shootdown(
                                    inner.addr + i * PAGE_SIZE,
                                    None,
                                    Some(new_flag),
                                )
                                .ignore()
                                .unwrap();
                        }
                        RangeChangeOp::Unmap => {
                            pg_table
                                .unmap_no_shootdown(inner.addr + i * PAGE_SIZE)
                                .ignore()
                                .unwrap();
                        }
                    };
                }
                pg_table.remote_flush_all();
            }
        }
    }

    /// Handle page fault happened on this VmMapping.
    pub(crate) fn handle_page_fault(&self, vaddr: VirtAddr, access_flags: MMUFlags) -> ZxResult {
        let vaddr = round_down_pages(vaddr);
        // Resolved BEFORE taking `self.inner`: it acquires the VMO family lock,
        // and the established discipline is to never hold the mapping lock
        // while taking a VMO lock (the reverse direction exists via
        // `range_change`, which only survives that nesting by using try_lock).
        let (vmo_offset, mut flags) = {
            let inner = self.inner.lock();
            let offset = vaddr - inner.addr;
            let page_idx = offset / PAGE_SIZE;
            (offset + inner.vmo_offset, inner.flags[page_idx])
        };
        // error!("page fault: addr = {:x}, access_flag = {:?}, flags = {:?}", vaddr, access_flags, flags);
        if !flags.contains(access_flags) {
            return Err(ZxError::ACCESS_DENIED);
        }
        // 当 PF 发生的时候，如果只要求读权限，则即便可写也现只给读权限。
        // 这是由于 COW 会去掉写权限来在写的时候触发 PF，如果直接把 flags 放进去，会导致 COW 的这个操作失效。
        if !access_flags.contains(MMUFlags::WRITE) {
            flags.remove(MMUFlags::WRITE);
        }
        let paddr = self.vmo.commit_page(vmo_offset / PAGE_SIZE, access_flags)?;
        // error!("paddr = {:x}", paddr);
        {
            // RE-CHECK under the mapping lock that this mapping still covers
            // `vaddr`, and covers it at the same VMO offset, before installing
            // the PTE.
            //
            // The window is real and was observed live: the mapping lock is
            // released above, and `Vmar::handle_page_fault` deliberately drops
            // the VMAR lock too (it only clones the Arc), so a concurrent
            // munmap / MAP_FIXED / mremap / execve can run `cut()` on this very
            // mapping while `commit_page` is off doing a (polled, multi-ms)
            // disk read. `cut()` unmaps the range and then narrows or zeroes
            // addr/size -- and we would come back and map the page ANYWAY,
            // leaving a live writable PTE with no VmMapping behind it.
            //
            // That orphan is not theoretical: an xfsettingsd fault dump showed
            // 0x507e490 resolving to pa=0xc249000 with READ|WRITE|USER while
            // the VMAR held nothing between 0x506a000 and 0x5080000, and the
            // surviving neighbour [0x5080000,0x5082000) carried vmo_offset=0x2000
            // over a vmo_len=0x4000 -- the signature of exactly this prefix cut.
            // Userspace then writes through the orphan PTE into a frame the
            // kernel considers free, and dies at the first page past it.
            //
            // Lock order is inner -> page_table, matching `cut()`; never the
            // reverse. Returning Ok simply retries the faulting instruction:
            // if a mapping still covers the address the retry resolves it, and
            // if none does, Vmar::handle_page_fault answers NOT_FOUND and the
            // process takes an honest SIGSEGV.
            let inner = self.inner.lock();
            if inner.size == 0
                || vaddr < inner.addr
                || vaddr >= inner.end_addr()
                || (vaddr - inner.addr) + inner.vmo_offset != vmo_offset
            {
                return Ok(());
            }
            let mut pg_table = self.page_table.lock();
            let mut res = pg_table.map(Page::new_aligned(vaddr, PageSize::Size4K), paddr, flags);
            if let Err(PagingError::AlreadyMapped) = res {
                res = pg_table.update(vaddr, Some(paddr), Some(flags)).map(|_| ());
            }
            res.map_err(|_| ZxError::ACCESS_DENIED)?;
        }
        // Fault-around: a non-write fault on a demand-paged region is almost
        // always the first of a run (code executing forward through a just-
        // mapped library, a reader walking a file mapping), and each miss costs
        // a full user trap + VMAR walk + commit + PTE install. Pre-commit and
        // pre-map the next few pages read-only so the run takes 1 trap per
        // 16 pages instead of per page. Best-effort: any error just stops it.
        if !access_flags.contains(MMUFlags::WRITE) {
            self.fault_around(vaddr);
        }
        Ok(())
    }

    /// Opportunistically commit + map the pages after `vaddr` (which the
    /// caller just resolved) with their WRITE bit removed, mirroring the
    /// CoW-preserving trick of the read-fault path: a later write still
    /// faults and goes through `commit_page(WRITE)` for its copy.
    ///
    /// Lock discipline matches `handle_page_fault`: candidates are gathered
    /// under the mapping lock, commits run with NO mapping lock held (they
    /// take the VMO family lock), and the PTE installs re-validate the
    /// mapping under the lock before touching the page table — a concurrent
    /// `cut()` makes this a silent no-op. Pages already mapped are SKIPPED
    /// (never downgraded: an existing PTE may carry a write-upgrade).
    fn fault_around(&self, vaddr: VirtAddr) {
        /// Pages mapped per fault including the faulting one (Linux's default
        /// `fault_around_bytes` is 64 KiB — the same 16 pages).
        const FAULT_AROUND_PAGES: usize = 16;
        let mut targets = [(0usize, 0usize, MMUFlags::empty()); FAULT_AROUND_PAGES - 1];
        let mut n = 0;
        let (snap_addr, snap_vmo_offset) = {
            let inner = self.inner.lock();
            if inner.size == 0 || vaddr < inner.addr || vaddr >= inner.end_addr() {
                return;
            }
            for i in 1..FAULT_AROUND_PAGES {
                let va = vaddr + i * PAGE_SIZE;
                if va >= inner.end_addr() {
                    break;
                }
                let page_idx = (va - inner.addr) / PAGE_SIZE;
                let mut flags = inner.flags[page_idx];
                if !flags.contains(MMUFlags::READ) {
                    break; // PROT_NONE tail: nothing mappable further on
                }
                flags.remove(MMUFlags::WRITE);
                targets[n] = (va, (va - inner.addr + inner.vmo_offset) / PAGE_SIZE, flags);
                n += 1;
            }
            (inner.addr, inner.vmo_offset)
        };
        if n == 0 {
            return;
        }
        // Commit with READ access only: on a CoW clone this reads through to
        // the parent without forcing a copy, exactly like a read fault.
        let mut committed = [(0usize, 0usize, MMUFlags::empty()); FAULT_AROUND_PAGES - 1];
        let mut m = 0;
        for &(va, vmo_page, flags) in &targets[..n] {
            match self.vmo.commit_page(vmo_page, MMUFlags::READ) {
                Ok(paddr) => {
                    committed[m] = (va, paddr, flags);
                    m += 1;
                }
                Err(_) => break,
            }
        }
        if m == 0 {
            return;
        }
        let inner = self.inner.lock();
        // Same re-validation as the primary page: a concurrent unmap /
        // MAP_FIXED may have cut or moved this mapping while we committed.
        if inner.size == 0 || inner.addr != snap_addr || inner.vmo_offset != snap_vmo_offset {
            return;
        }
        let mut pg_table = self.page_table.lock();
        for &(va, paddr, flags) in &committed[..m] {
            if va >= inner.end_addr() {
                break;
            }
            // Fresh installs only; AlreadyMapped (or any other error) is left
            // exactly as it was.
            let _ = pg_table.map(Page::new_aligned(va, PageSize::Size4K), paddr, flags);
        }
    }

    /// Clone VMO and map it to a new page table. (For Linux fork)
    ///
    /// Independent copy — NOT copy-on-write. The Zircon-style hidden-node COW
    /// tree mis-replicated the address space for a process that *runs* (rather
    /// than immediately `execve`s) after `fork`: openrc-init and its forked
    /// children faulted with SIGSEGV on writes to pages that fork should have
    /// provided (NULL / unmapped heap-BSS writes, and writes to pages whose COW
    /// copy never happened), while busybox — which always fork+exec's — never
    /// exercised it. We give the child a straight, independent copy of the
    /// parent VMO's current contents instead. It costs more memory and copy time
    /// than COW, but it is correct, and it leaves the PARENT completely untouched
    /// (no COW write-protect of its pages), so the forking process keeps writing
    /// to its own frames without faulting.
    ///
    /// "Current contents" only requires copying pages the parent has actually
    /// committed: an untouched page reads back exactly as it will re-fault in
    /// the child (file bytes for a demand-paged mapping, zeros for anonymous
    /// memory), so `fork_copy` skips it and keeps it lazy. Only when the VMO
    /// can't guarantee that (slices, pinned/contiguous, clone trees) do we fall
    /// back to the eager full read-copy.
    /// Try to give `fork` a copy-on-write snapshot of this mapping's VMO
    /// instead of a copy of its pages. `None` means the caller must fall back
    /// to the eager path.
    ///
    /// `fork_copy`, the previous behaviour, walks every resident frame,
    /// allocates a new one and `pmem_copy`s 4 KiB into it — `fork` is O(resident
    /// memory), so a process holding 100 MiB pays a 100 MiB copy every time it
    /// forks. Measured against Linux under the same emulator, forking with
    /// 16 MiB resident cost 70 ms here against 9.4 ms there, and the cost per
    /// resident MiB was 4.2x what a `memcpy` of that MiB costs on the same
    /// machine (the copy, plus a frame allocation per page, plus the page-table
    /// build). See docs/README-performance.md.
    ///
    /// `VmObject::create_child` is the Zircon snapshot clone: it moves this
    /// VMO's frames into a hidden shared parent, points both this VMO and the
    /// new child at it, and write-protects every existing mapping so the first
    /// write on either side faults and copies just that page.
    ///
    /// Two cases deliberately keep the old eager path:
    ///
    /// * **A VMO with more than one mapper.** `create_child` write-protects
    ///   *every* mapping of the object, which for a genuinely shared mapping
    ///   would silently convert another process's shared writes into private
    ///   copy-on-write ones. Forking a shared mapping is already wrong here
    ///   (the child gets a private copy either way); this keeps it exactly as
    ///   wrong as it was rather than adding a new way to break it.
    /// * **Anything `create_child` rejects** — contiguous, pinned or
    ///   non-cached objects — which it reports as an error.
    /// [`try_cow_child`](Self::try_cow_child) with its two halves timed
    /// separately.
    ///
    /// A copy-on-write fork of a process with 256 small mappings costs 4.9 ms on
    /// its FIRST fork -- the same order as Linux -- and 27 to 110 ms on every
    /// one after, resetting when the process is replaced by `exec`. The census
    /// in `/proc/perf/kernel` rules out the obvious explanations: the hidden
    /// nodes all collapse (0 live), no VMO leaks, and every fork takes the
    /// copy-on-write path rather than falling back to the eager copy. So the
    /// cost is inside one of these steps and grows for reasons the object graph
    /// does not show. These timers say which step, which is the one fact that
    /// cannot be deduced from the structure.
    fn try_cow_child_timed(&self) -> Option<Arc<VmObject>> {
        if !COW_FORK.load(Ordering::Relaxed) {
            return None;
        }
        if self.vmo.share_count() > 1 {
            return None;
        }
        let t0 = kernel_hal::timer::timer_now().as_nanos() as u64;
        let child = self.vmo.create_child(false, 0, self.vmo.len());
        let t1 = kernel_hal::timer::timer_now().as_nanos() as u64;
        FORK_NS_CREATE_CHILD.fetch_add(t1 - t0, Ordering::Relaxed);
        let child = child.ok()?;
        self.protect_for_cow();
        let t2 = kernel_hal::timer::timer_now().as_nanos() as u64;
        FORK_NS_PROTECT.fetch_add(t2 - t1, Ordering::Relaxed);
        return Some(child);
    }

    #[allow(dead_code)]
    fn try_cow_child(&self) -> Option<Arc<VmObject>> {
        if !COW_FORK.load(Ordering::Relaxed) {
            return None;
        }
        if self.vmo.share_count() > 1 {
            return None;
        }
        let child = self.vmo.create_child(false, 0, self.vmo.len()).ok()?;
        // `create_child` already asked every mapping of the parent VMO to drop
        // WRITE, but `VmMapping::range_change` uses `try_lock` and silently
        // skips a mapping whose lock is held right then — fine for its original
        // callers (the fault path re-installs the PTE afterwards), NOT fine
        // here: a missed mapping would leave the parent with writable PTEs onto
        // frames the child now shares, which is silent cross-process
        // corruption. A sibling thread of the forking process faulting on this
        // very mapping is exactly the race. Re-apply it with a blocking lock —
        // `clone_map` provably holds neither this mapping's `inner` nor the
        // page table, so blocking here cannot deadlock, and the operation is
        // idempotent with what `create_child` already did.
        self.protect_for_cow();
        Some(child)
    }

    /// Drop WRITE from every PTE of this mapping, taking the lock rather than
    /// skipping on contention. See [`VmMapping::try_cow_child`].
    fn protect_for_cow(&self) {
        let inner = self.inner.lock();
        let mut pg_table = self.page_table.lock();
        // `flags.len()` — which is `pages(size)`, rounded UP (see the
        // `VmMapping` constructor) — NOT `size / PAGE_SIZE`, which rounds down.
        // On a mapping whose size is not a whole number of pages the two differ
        // by one, and the page they differ by is the last one: rounding down
        // would leave it writable in the parent while the child already shares
        // its frame, which is precisely the corruption this function exists to
        // prevent. (`map_committed` has the same rounding-down habit; that is
        // pre-existing and left alone here.)
        let pages = inner.flags.len();
        // Deliberately unconditional. `create_child` has normally just done
        // this, but it uses `try_lock` and silently skips a mapping whose lock
        // is held right then, so this pass is the guarantee rather than an
        // optimisation — and the thing it guarantees is that no page stays
        // writable in the parent while the child shares its frame.
        //
        // Skipping pages whose PTE already reads non-writable (querying first)
        // was tried and reverted: it did not clearly pay for itself, and the one
        // pass that has to be exhaustive is a bad place to trade certainty for
        // a second read of the same state.
        for i in 0..pages {
            let mut flags = inner.flags[i];
            if !flags.contains(MMUFlags::WRITE) {
                continue; // mapping is not writable at all
            }
            flags.remove(MMUFlags::WRITE);
            pg_table
                .update_no_shootdown(inner.addr + i * PAGE_SIZE, None, Some(flags))
                .ignore()
                .unwrap();
        }
        // mmu-gather: one cross-CPU shootdown for the whole mapping, never one
        // per page (see `protect`).
        if pages > 0 {
            pg_table.remote_flush_all();
        }
    }

    fn clone_map(
        &self,
        page_table: Arc<Mutex<dyn GenericPageTable>>,
        cow_cache: &mut BTreeMap<u64, Arc<VmObject>>,
    ) -> ZxResult<Arc<Self>> {
        let _phase = ForkPhase::start();
        // Fast path: copy only the pages the parent has actually committed.
        // Untouched pages of a file-backed mapping stay lazy in the child and
        // fault in from the same file later; untouched anonymous pages stay
        // demand-zero. Without this, forking a process with large demand-paged
        // libraries (a Wayland compositor maps 100+ MiB of wlroots/Mesa) read
        // EVERY byte of EVERY mapped file from disk 4 KiB at a time — minutes
        // of wall-clock on polled AHCI — only for the child to execve() and
        // throw the copy away.
        // Lazy fast path: copy only the pages the parent has actually
        // committed; untouched pages of a file-backed mapping stay lazy in the
        // child and fault in from the same file later. Without this, forking a
        // compositor eagerly reads its ENTIRE mapped set (251 MiB across ~600
        // mappings observed on real hardware) from the polled disk, 4 KiB at a
        // time — many minutes per fork, unusable.
        //
        // History: this fast path was previously reverted because forks raced
        // the COW machinery with a "RefCell already mutably borrowed" panic in
        // vmo/paged.rs. The actual root cause was a drop-order bug in
        // get_inner/get_inner_mut: every call site destructures the returned
        // (borrow, guard) tuple, and pattern bindings drop in REVERSE order,
        // so the family mutex was released a few instructions BEFORE the
        // RefCell borrow died — any concurrent vmo op could then panic. That
        // is now fixed at the source (guard first, borrow second — see
        // paged.rs get_inner), making this fast path sound again.
        let new_vmo = if self.vmo.is_physical() || self.vmo.is_share_on_fork() {
            // A physical/device window (dumb buffer, dma-buf, or framebuffer
            // mapped via `new_physical`) is NOT per-process memory: it is a view
            // onto a fixed physical range. `fork` must SHARE it — both processes
            // see the same pixels — so hand the child a mapping over the SAME
            // VMObject. Copying it was doubly wrong: the child got a private
            // stale snapshot, and the eager fallback below read the range page-
            // by-page with `pmem_read`, which on a BAR/device range is
            // catastrophically slow and wedged the machine mid-fork (observed:
            // labwc's fork of swaybg/foot froze copying a mapping in the middle
            // of its 596-mapping address space).
            //
            // `is_share_on_fork` is the same rule for Linux MAP_SHARED memory —
            // anonymous MAP_SHARED, shared file mappings, SysV shm: the child
            // must see the parent's stores and vice versa. Copying these was a
            // silent ABI break (both SMP fairness probes read zero progress
            // from their hogs because the counters lived in a "shared" page
            // the fork had quietly privatized).
            self.vmo.clone()
        } else if let Some(existing) = cow_cache.get(&self.vmo.id()) {
            // A sibling mapping already produced the child for this source VMO
            // (a COW snapshot, or an eager copy when `create_child` is refused —
            // e.g. `share_count > 1`, which is exactly the many-mappings-one-VMO
            // case an mprotect/munmap split creates). Doing it per mapping is
            // what made fork O(M²): M full copies of one VMO, or M redundant
            // `create_child` mapper-walks. Reusing the one child collapses that
            // to O(M), and siblings sharing it also preserves the aliasing the
            // mappings had in the parent.
            existing.clone()
        } else {
            let child = if let Some(cow) = self.try_cow_child_timed() {
                cow
            } else {
                self.fork_eager_copy()?
            };
            cow_cache.insert(self.vmo.id(), child.clone());
            child
        };
        let mapping = Arc::new(VmMapping {
            inner: Mutex::new(self.inner.lock().clone()),
            permissions: self.permissions,
            page_table,
            vmo: new_vmo.clone(),
        });
        new_vmo.append_mapping(Arc::downgrade(&mapping));
        Ok(mapping)
    }

    /// Eager fork fallback: a private copy of this mapping's VMO, used when
    /// `create_child` is refused (contiguous, pinned, page-cache borrower, or a
    /// VMO with more than one mapper). Factored out of `clone_map` so the
    /// per-VMO dedup cache can wrap it — the many-mappings-over-one-VMO case
    /// lands here, and copying the whole VMO once per mapping is O(M²).
    fn fork_eager_copy(&self) -> ZxResult<Arc<VmObject>> {
        let eager_t0 = kernel_hal::timer::timer_now().as_nanos() as u64;
        FORK_EAGER_MAPPINGS.fetch_add(1, Ordering::Relaxed);
        FORK_EAGER_BYTES.fetch_add(self.vmo.len() as u64, Ordering::Relaxed);
        let _eager = EagerPhase(eager_t0);
        Ok(match self.vmo.fork_copy() {
            Ok(vmo) => vmo,
            Err(_) => {
                // Eager full copy fallback (slices, contiguous, clone trees,
                // pinned or non-Origin vmos). `read` commits / fills from the
                // backing source as needed; `write` commits the fresh child
                // frames.
                warn!(
                    "fork: eager fallback copy of vmo len={:#x} (fork_copy unsupported)",
                    self.vmo.len()
                );
                let len = self.vmo.len();
                // `pages()` (round UP), not `len / PAGE_SIZE`: integer
                // division silently drops the final partial page, handing
                // the child a VMO one page SHORTER than the mapping that
                // will point at it. The child's mapping keeps the parent's
                // `size`, so its last page has no backing store and faults
                // as unmapped memory the moment it is touched -- a
                // heap/library region that simply ends early in the child.
                let new_vmo = VmObject::new_paged(pages(len));
                let mut buf = vec![0u8; PAGE_SIZE];
                let mut off = 0;
                while off < len {
                    // The final chunk may be short; clamp so the read stays
                    // inside the source VMO and the write inside the new one.
                    let n = PAGE_SIZE.min(len - off);
                    self.vmo.read(off, &mut buf[..n])?;
                    new_vmo.write(off, &buf[..n])?;
                    off += PAGE_SIZE;
                }
                new_vmo
            }
        })
    }
}

impl VmMappingInner {
    fn end_addr(&self) -> VirtAddr {
        self.addr + self.size
    }
}

impl Drop for VmMapping {
    fn drop(&mut self) {
        let inner = self.inner.lock();
        info!(
            "VmMapping::drop: addr={:#x}, size={:#x}",
            inner.addr, inner.size
        );
        drop(inner);
        self.unmap();
    }
}

/// The base of kernel address space
/// In x86 fuchsia this is 0xffff_ff80_0000_0000 instead
#[cfg(target_os = "none")]
pub const KERNEL_ASPACE_BASE: u64 = 0xffff_ff02_0000_0000;
/// Hosted (libos) builds run in a normal user process: keep the "kernel"
/// aspace inside the host's mappable range, above the mock PMEM window.
#[cfg(not(target_os = "none"))]
pub const KERNEL_ASPACE_BASE: u64 = 0x0000_0010_0000_0000;
/// The size of kernel address space
#[cfg(target_os = "none")]
pub const KERNEL_ASPACE_SIZE: u64 = 0x0000_0080_0000_0000;
/// Hosted (libos) kernel aspace size.
#[cfg(not(target_os = "none"))]
pub const KERNEL_ASPACE_SIZE: u64 = 0x0000_0010_0000_0000;
/// The base of user address space
pub const USER_ASPACE_BASE: u64 = 0;
// pub const USER_ASPACE_BASE: u64 = 0x0000_0000_0100_0000;
/// The size of user address space
pub const USER_ASPACE_SIZE: u64 = (1u64 << 47) - 4096 - USER_ASPACE_BASE;
/// The default number of user stack pages
pub const USER_STACK_PAGES: usize = 128;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_child() {
        let root_vmar = VmAddressRegion::new_root();
        let child = root_vmar
            .allocate_at(0, 0x2000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
            .expect("failed to create child VMAR");

        // test invalid argument
        assert_eq!(
            root_vmar
                .allocate_at(0x2001, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            root_vmar
                .allocate_at(0x2000, 1, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            root_vmar
                .allocate_at(0, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .err(),
            Some(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            child
                .allocate_at(0x1000, 0x2000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .err(),
            Some(ZxError::INVALID_ARGS)
        );
    }

    /// A valid virtual address base to mmap.
    const MAGIC: usize = 0xdead_beaf;

    #[test]
    #[allow(unsafe_code)]
    fn map() {
        let vmar = VmAddressRegion::new_root();
        let vmo = VmObject::new_paged(4);
        let flags = MMUFlags::READ | MMUFlags::WRITE;

        // invalid argument
        assert_eq!(
            vmar.map_at(0, vmo.clone(), 0x4000, 0x1000, flags),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.map_at(0, vmo.clone(), 0, 0x5000, flags),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.map_at(0, vmo.clone(), 0x1000, 1, flags),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.map_at(0, vmo.clone(), 1, 0x1000, flags),
            Err(ZxError::INVALID_ARGS)
        );

        vmar.map_at(0, vmo.clone(), 0, 0x4000, flags).unwrap();
        vmar.map_at(0x12000, vmo.clone(), 0x2000, 0x1000, flags)
            .unwrap();

        unsafe {
            ((vmar.addr() + 0x2000) as *mut usize).write(MAGIC);
            assert_eq!(((vmar.addr() + 0x12000) as *const usize).read(), MAGIC);
        }
    }

    /// ```text
    /// +--------+--------+--------+--------+
    /// |           root              ....  |
    /// +--------+--------+--------+--------+
    /// |      child1     | child2 |
    /// +--------+--------+--------+
    /// | g-son1 | g-son2 |
    /// +--------+--------+
    /// ```
    struct Sample {
        root: Arc<VmAddressRegion>,
        child1: Arc<VmAddressRegion>,
        child2: Arc<VmAddressRegion>,
        grandson1: Arc<VmAddressRegion>,
        grandson2: Arc<VmAddressRegion>,
    }

    impl Sample {
        fn new() -> Self {
            let root = VmAddressRegion::new_root();
            let child1 = root
                .allocate_at(0, 0x2000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .unwrap();
            let child2 = root
                .allocate_at(0x2000, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .unwrap();
            let grandson1 = child1
                .allocate_at(0, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .unwrap();
            let grandson2 = child1
                .allocate_at(0x1000, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .unwrap();
            Sample {
                root,
                child1,
                child2,
                grandson1,
                grandson2,
            }
        }
    }

    #[test]
    fn unmap_vmar() {
        let s = Sample::new();
        let base = s.root.addr();
        s.child1.unmap(base, 0x1000).unwrap();
        assert!(s.grandson1.is_dead());
        assert!(s.grandson2.is_alive());

        // partial overlap sub-region should fail.
        let s = Sample::new();
        let base = s.root.addr();
        assert_eq!(
            s.root.unmap(base + 0x1000, 0x2000),
            Err(ZxError::INVALID_ARGS)
        );

        // unmap nothing should success.
        let s = Sample::new();
        let base = s.root.addr();
        s.child1.unmap(base + 0x8000, 0x1000).unwrap();
    }

    #[test]
    fn destroy() {
        let s = Sample::new();
        s.child1.destroy().unwrap();
        assert!(s.child1.is_dead());
        assert!(s.grandson1.is_dead());
        assert!(s.grandson2.is_dead());
        assert!(s.child2.is_alive());
        // address space should be released
        assert!(s
            .root
            .allocate_at(0, 0x1000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
            .is_ok());
    }

    #[test]
    fn unmap_mapping() {
        //   +--------+--------+--------+--------+--------+
        // 1 [--------------------------|xxxxxxxx|--------]
        // 2 [xxxxxxxx|-----------------]
        // 3          [--------|xxxxxxxx]
        // 4          [xxxxxxxx]
        let vmar = VmAddressRegion::new_root();
        let base = vmar.addr();
        let vmo = VmObject::new_paged(5);
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        vmar.map_at(0, vmo, 0, 0x5000, flags).unwrap();
        assert_eq!(vmar.count(), 1);
        assert_eq!(vmar.used_size(), 0x5000);

        // 0. unmap none.
        vmar.unmap(base + 0x5000, 0x1000).unwrap();
        assert_eq!(vmar.count(), 1);
        assert_eq!(vmar.used_size(), 0x5000);

        // 1. unmap middle.
        vmar.unmap(base + 0x3000, 0x1000).unwrap();
        assert_eq!(vmar.count(), 2);
        assert_eq!(vmar.used_size(), 0x4000);

        // 2. unmap prefix.
        vmar.unmap(base, 0x1000).unwrap();
        assert_eq!(vmar.count(), 2);
        assert_eq!(vmar.used_size(), 0x3000);

        // 3. unmap postfix.
        vmar.unmap(base + 0x2000, 0x1000).unwrap();
        assert_eq!(vmar.count(), 2);
        assert_eq!(vmar.used_size(), 0x2000);

        // 4. unmap all.
        vmar.unmap(base + 0x1000, 0x1000).unwrap();
        assert_eq!(vmar.count(), 1);
        assert_eq!(vmar.used_size(), 0x1000);
    }

    #[test]
    #[allow(unsafe_code)]
    fn copy_on_write_update_mapping() {
        let vmar = VmAddressRegion::new_root();
        let vmo = VmObject::new_paged(1);
        vmo.test_write(0, 1);
        vmar.map_at(0, vmo.clone(), 0, PAGE_SIZE, MMUFlags::RXW)
            .unwrap();
        let child_vmo = vmo.create_child(false, 0, 1 * PAGE_SIZE).unwrap();
        // The clone was created after the map, so the two vmo share pages.
        assert_eq!(
            vmo.commit_page(0, MMUFlags::READ),
            child_vmo.commit_page(0, MMUFlags::READ)
        );
        assert_eq!(vmo.test_read(0), 1);
        assert_eq!(child_vmo.test_read(0), 1);
        unsafe {
            assert_eq!((vmar.addr() as *const u8).read(), 1);
        }
        vmo.test_write(0, 2);
        // Here, since the page was copied on write, the actual page used in the vmo should be changed.
        assert_ne!(
            vmo.commit_page(0, MMUFlags::READ),
            child_vmo.commit_page(0, MMUFlags::READ)
        );
        assert_eq!(vmo.test_read(0), 2);
        assert_eq!(child_vmo.test_read(0), 1);
        // The mapping should update to reflect this change.
        // Since we do not have page fault handler in the libOS,
        // so manually simulate the page fault before read to it
        vmar.handle_page_fault(vmar.addr(), MMUFlags::READ).unwrap();
        unsafe {
            assert_eq!((vmar.addr() as *const u8).read(), 2);
        }
    }

    #[test]
    fn remap_shrink_in_place() {
        let vmar = VmAddressRegion::new_root();
        let base = vmar.addr();
        let vmo = VmObject::new_paged(4);
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        vmar.map_at(0, vmo, 0, 0x4000, flags).unwrap();

        let ret = vmar.remap(base, 0x4000, 0x2000, false, None).unwrap();
        assert_eq!(ret, base);
        assert_eq!(vmar.used_size(), 0x2000);
        // Same-size call is a no-op.
        let ret = vmar.remap(base, 0x2000, 0x2000, false, None).unwrap();
        assert_eq!(ret, base);
        assert_eq!(vmar.used_size(), 0x2000);
    }

    #[test]
    fn remap_grow_in_place() {
        let vmar = VmAddressRegion::new_root();
        let base = vmar.addr();
        let vmo = VmObject::new_paged(2);
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        vmar.map_at(0, vmo, 0, 0x2000, flags).unwrap();

        // Nothing after the mapping: growth extends without moving, even
        // without MAYMOVE, by attaching an anonymous demand-paged tail.
        let ret = vmar.remap(base, 0x2000, 0x4000, false, None).unwrap();
        assert_eq!(ret, base);
        assert_eq!(vmar.used_size(), 0x4000);
    }

    #[test]
    #[allow(unsafe_code)]
    fn remap_move_preserves_content() {
        let vmar = VmAddressRegion::new_root();
        let base = vmar.addr();
        let vmo = VmObject::new_paged(2);
        vmo.test_write(0, 42);
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        vmar.map_at(0, vmo, 0, 0x2000, flags).unwrap();
        // Block the address right after so in-place growth is impossible.
        let blocker = VmObject::new_paged(1);
        vmar.map_at(0x2000, blocker, 0, 0x1000, flags).unwrap();

        // Without MAYMOVE the grow must fail...
        assert_eq!(
            vmar.remap(base, 0x2000, 0x4000, false, None),
            Err(ZxError::NO_MEMORY)
        );
        // ...with MAYMOVE it relocates, keeping the bytes (same VmObject).
        let new_addr = vmar.remap(base, 0x2000, 0x4000, true, None).unwrap();
        assert_ne!(new_addr, base);
        unsafe {
            assert_eq!((new_addr as *const u8).read(), 42);
        }
        // The old range is gone: remapping it again reports no mapping.
        assert_eq!(
            vmar.remap(base, 0x2000, 0x2000, true, None),
            Err(ZxError::NOT_FOUND)
        );
    }

    #[test]
    #[allow(unsafe_code)]
    fn remap_fixed_target() {
        let vmar = VmAddressRegion::new_root();
        let base = vmar.addr();
        let vmo = VmObject::new_paged(1);
        vmo.test_write(0, 7);
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        vmar.map_at(0, vmo, 0, 0x1000, flags).unwrap();

        // Target overlapping the old range is invalid.
        assert_eq!(
            vmar.remap(base, 0x1000, 0x1000, true, Some(base)),
            Err(ZxError::INVALID_ARGS)
        );
        let target = base + 0x8000;
        let ret = vmar
            .remap(base, 0x1000, 0x2000, true, Some(target))
            .unwrap();
        assert_eq!(ret, target);
        unsafe {
            assert_eq!((target as *const u8).read(), 7);
        }
    }

    #[test]
    fn mappings_dump_lists_sorted_rows() {
        let vmar = VmAddressRegion::new_root();
        let base = vmar.addr();
        let rw = MMUFlags::READ | MMUFlags::WRITE;
        let ro = MMUFlags::READ;
        // Map out of order to check the dump sorts by start address.
        vmar.map_at(0x3000, VmObject::new_paged(1), 0, 0x1000, ro)
            .unwrap();
        vmar.map_at(0, VmObject::new_paged(2), 0, 0x2000, rw)
            .unwrap();

        let rows = vmar.mappings_dump();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].start, base);
        assert_eq!(rows[0].end, base + 0x2000);
        assert!(rows[0].flags.contains(MMUFlags::WRITE));
        assert!(!rows[0].shared);
        assert_eq!(rows[1].start, base + 0x3000);
        assert!(!rows[1].flags.contains(MMUFlags::WRITE));

        // A shared VMO (second mapper) flips the shared bit.
        let vmo = VmObject::new_paged(1);
        vmar.map_at(0x5000, vmo.clone(), 0, 0x1000, rw).unwrap();
        vmar.map_at(0x7000, vmo, 0, 0x1000, rw).unwrap();
        let rows = vmar.mappings_dump();
        assert_eq!(rows.len(), 4);
        assert!(rows[2].shared && rows[3].shared);
    }

    #[test]
    fn remap_rejects_bad_args() {
        let vmar = VmAddressRegion::new_root();
        let base = vmar.addr();
        let vmo = VmObject::new_paged(2);
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        vmar.map_at(0, vmo, 0, 0x2000, flags).unwrap();

        // Unaligned old address / zero sizes.
        assert_eq!(
            vmar.remap(base + 1, 0x1000, 0x1000, true, None),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.remap(base, 0, 0x1000, true, None),
            Err(ZxError::INVALID_ARGS)
        );
        assert_eq!(
            vmar.remap(base, 0x1000, 0, true, None),
            Err(ZxError::INVALID_ARGS)
        );
        // Old range spanning past its mapping: no single covering mapping.
        assert_eq!(
            vmar.remap(base, 0x3000, 0x3000, true, None),
            Err(ZxError::NOT_FOUND)
        );
    }

    /// Kernel-chosen placement (`map(None, ..)`) packs upward with no overlap,
    /// and every placed page resolves through `find_mapping`. This is the fast
    /// path — each mapping lands just above the previous one — and the
    /// `debug_assert!(test_map(..))` inside `find_free_area` fires if a returned
    /// slot is ever occupied.
    #[test]
    fn auto_placement_packs_upward() {
        let vmar = VmAddressRegion::new_root();
        let base = vmar.addr();
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        let mut addrs = Vec::new();
        for _ in 0..64 {
            let a = vmar
                .map(None, VmObject::new_paged(1), 0, 0x1000, flags)
                .unwrap();
            assert!(!addrs.contains(&a), "auto-placement returned an occupied slot");
            addrs.push(a);
        }
        addrs.sort_unstable();
        assert_eq!(addrs[0], base);
        for w in addrs.windows(2) {
            assert_eq!(w[1] - w[0], 0x1000);
        }
        assert_eq!(vmar.count(), 64);
        for &a in &addrs {
            assert!(vmar.find_mapping(a).is_some());
        }
        assert!(vmar.find_mapping(base + 64 * 0x1000).is_none());
    }

    /// When the space above the top mapping is exhausted, placement falls back
    /// to first-fit and reuses a hole left by `unmap`. A small child arena makes
    /// "exhausted" reachable in a test.
    #[test]
    fn auto_placement_reuses_holes_when_full() {
        let root = VmAddressRegion::new_root();
        let arena = root
            .allocate_at(0, 0x10000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
            .unwrap();
        let base = arena.addr();
        let flags = MMUFlags::READ | MMUFlags::WRITE;

        for _ in 0..16 {
            arena
                .map(None, VmObject::new_paged(1), 0, 0x1000, flags)
                .unwrap();
        }
        // Arena full: the top is exhausted and there is no hole.
        assert_eq!(
            arena.map(None, VmObject::new_paged(1), 0, 0x1000, flags),
            Err(ZxError::NO_MEMORY)
        );

        // Open a hole in the middle; the next placement must reuse exactly it.
        let hole = base + 0x8000;
        arena.unmap(hole, 0x1000).unwrap();
        let reused = arena
            .map(None, VmObject::new_paged(1), 0, 0x1000, flags)
            .unwrap();
        assert_eq!(reused, hole);
    }

    /// First-fit must step over a child sub-VMAR, never place across it.
    #[test]
    fn auto_placement_skips_children() {
        let root = VmAddressRegion::new_root();
        let base = root.addr();
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        let _child = root
            .allocate_at(0x2000, 0x2000, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
            .unwrap();
        // A 4-page region cannot start at 0 (it would cross the child at
        // 0x2000), so it must be placed at or above the child's end.
        let a = root
            .map(None, VmObject::new_paged(4), 0, 0x4000, flags)
            .unwrap();
        assert!(a >= base + 0x4000);
    }

    /// Cutting the FRONT of a mapping moves its start address — its BTreeMap key
    /// — so `unmap` must reinsert the survivor under the new key or later
    /// lookups miss a live region.
    #[test]
    fn unmap_prefix_reinserts_under_new_key() {
        let vmar = VmAddressRegion::new_root();
        let base = vmar.addr();
        let flags = MMUFlags::READ | MMUFlags::WRITE;
        vmar.map_at(0, VmObject::new_paged(4), 0, 0x4000, flags)
            .unwrap();
        // Remove the first two pages: the mapping now starts at base+0x2000.
        vmar.unmap(base, 0x2000).unwrap();
        assert!(vmar.find_mapping(base).is_none());
        assert!(vmar.find_mapping(base + 0x1000).is_none());
        assert!(vmar.find_mapping(base + 0x2000).is_some());
        assert!(vmar.find_mapping(base + 0x3000).is_some());
        let rows = vmar.mappings_dump();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].start, base + 0x2000);
        assert_eq!(rows[0].end, base + 0x4000);
    }
}

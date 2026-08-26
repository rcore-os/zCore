use {
    self::{paged::*, physical::*, slice::*},
    super::*,
    crate::object::*,
    alloc::{
        sync::{Arc, Weak},
        vec::Vec,
    },
    bitflags::bitflags,
    core::ops::Deref,
    core::sync::atomic::{AtomicUsize, Ordering},
    kernel_hal::CachePolicy,
    lock::{Mutex, MutexGuard},
};

mod paged;

pub use paged::{cow_tree_stats, mapping_list_stats};
mod physical;
mod slice;

kcounter!(VMO_PAGE_ALLOC, "vmo.page_alloc");
kcounter!(VMO_PAGE_DEALLOC, "vmo.page_dealloc");

/// The amount of memory committed to VMOs.
pub fn vmo_page_bytes() -> usize {
    (VMO_PAGE_ALLOC.get() - VMO_PAGE_DEALLOC.get()) * PAGE_SIZE
}

/// A lazy backing source for a paged VMO (demand paging).
///
/// Lets a file-backed `mmap` be paged in on demand: instead of eagerly reading
/// the whole file into the VMO at map time (which, for a ~150 MiB library like
/// `libLLVM.so` pulled in by `perf`, stalls the machine reading hundreds of MiB
/// synchronously), each page is read from the source the first time it is
/// committed — i.e. on the page fault that first touches it. A process only
/// pays for the pages it actually uses.
pub trait FrameFiller: Send + Sync {
    /// Number of source bytes available from the start of the VMO. Pages whose
    /// start offset is `>= source_len()` are plain zero pages (e.g. the BSS tail
    /// of a file mapping that extends past end-of-file).
    fn source_len(&self) -> usize;

    /// Fill `buf` (exactly one page) with the source bytes starting at byte
    /// `offset` within the VMO. The buffer is pre-zeroed; any bytes past
    /// `source_len()` must be left as zero.
    fn fill_page(&self, offset: usize, buf: &mut [u8]);
}

/// Virtual Memory Object Trait
#[allow(clippy::len_without_is_empty)]
pub trait VMObjectTrait: Sync + Send {
    /// Read memory to `buf` from VMO at `offset`.
    fn read(&self, offset: usize, buf: &mut [u8]) -> ZxResult;

    /// Write memory from `buf` to VMO at `offset`.
    fn write(&self, offset: usize, buf: &[u8]) -> ZxResult;

    /// Resets the range of bytes in the VMO from `offset` to `offset+len` to 0.
    fn zero(&self, offset: usize, len: usize) -> ZxResult;

    /// Get the length of VMO.
    fn len(&self) -> usize;

    /// Set the length of VMO.
    fn set_len(&self, len: usize) -> ZxResult;

    /// Commit a page.
    fn commit_page(&self, page_idx: usize, flags: MMUFlags) -> ZxResult<PhysAddr>;

    /// Commit pages with an external function f.
    /// the vmo is internally locked before it calls f,
    /// allowing `VmMapping` to avoid deadlock
    #[allow(clippy::type_complexity)]
    fn commit_pages_with(
        &self,
        f: &mut dyn FnMut(&mut dyn FnMut(usize, MMUFlags) -> ZxResult<PhysAddr>) -> ZxResult,
    ) -> ZxResult;

    /// Commit allocating physical memory.
    fn commit(&self, offset: usize, len: usize) -> ZxResult;

    /// Decommit allocated physical memory.
    fn decommit(&self, offset: usize, len: usize) -> ZxResult;

    /// Create a child VMO.
    fn create_child(&self, offset: usize, len: usize) -> ZxResult<Arc<dyn VMObjectTrait>>;

    /// Append a mapping to the VMO's mapping list.
    fn append_mapping(&self, _mapping: Weak<VmMapping>) {}

    /// Remove a mapping from the VMO's mapping list.
    fn remove_mapping(&self, _mapping: Weak<VmMapping>) {}

    /// Complete the VmoInfo.
    fn complete_info(&self, info: &mut VmoInfo);

    /// Get the cache policy.
    fn cache_policy(&self) -> CachePolicy;

    /// Set the cache policy.
    fn set_cache_policy(&self, policy: CachePolicy) -> ZxResult;

    /// Count committed pages of the VMO.
    fn committed_pages_in_range(&self, start_idx: usize, end_idx: usize) -> usize;

    /// Pin the given range of the VMO.
    fn pin(&self, _offset: usize, _len: usize) -> ZxResult {
        Err(ZxError::NOT_SUPPORTED)
    }

    /// Unpin the given range of the VMO.
    fn unpin(&self, _offset: usize, _len: usize) -> ZxResult {
        Err(ZxError::NOT_SUPPORTED)
    }

    /// Returns true if the object is backed by a contiguous range of physical memory.
    fn is_contiguous(&self) -> bool {
        false
    }

    /// Returns true if the object is backed by RAM.
    fn is_paged(&self) -> bool {
        false
    }

    /// Returns true if the object is a fixed window onto a specific physical
    /// range (device/BAR/contiguous buffer shared via `new_physical`), as
    /// opposed to owned RAM. Such an object must be SHARED across `fork`, never
    /// copied: it has no per-process contents, and eager-copying a device range
    /// page-by-page can wedge the machine.
    fn is_physical(&self) -> bool {
        false
    }

    /// Returns true if the object fills untouched pages lazily from a backing
    /// source (a file-backed mapping). Used to decide whether fault-around
    /// pre-commit is worthwhile.
    fn is_demand_paged(&self) -> bool {
        false
    }

    /// Physical address of `page_idx` IF it is already committed — WITHOUT
    /// committing it. Used by the lazy fork map: only committed pages get PTEs
    /// installed eagerly; untouched pages stay unmapped and demand-fault later
    /// (zero-fill or backing-source fill), exactly like the parent's did.
    /// Default `None` means "treat every page as uncommitted".
    fn committed_paddr(&self, _page_idx: usize) -> Option<PhysAddr> {
        None
    }

    /// Independent copy for Linux `fork`: committed pages are copied eagerly;
    /// pages never touched by the parent stay lazy in the child (refilled from
    /// the same backing source, or demand-zero). Implementations that cannot
    /// guarantee those semantics return `NOT_SUPPORTED` and the caller falls
    /// back to an eager full copy.
    fn fork_copy(&self) -> ZxResult<Arc<dyn VMObjectTrait>> {
        Err(ZxError::NOT_SUPPORTED)
    }

    /// If contiguous, transmute vmo to a mutable buffer
    fn as_mut_buf(&self) -> ZxResult<(MutexGuard<'_, ()>, &mut [u8])> {
        Err(ZxError::NOT_SUPPORTED)
    }

    /// Mark as not contiguous
    fn unset_contiguous(&self) {}
}

/// Virtual memory containers
///
/// ## SYNOPSIS
///
/// A Virtual Memory Object (VMO) represents a contiguous region of virtual memory
/// that may be mapped into multiple address spaces.
pub struct VmObject {
    base: KObjectBase,
    _counter: CountHelper,
    /// Backing kind, for the per-kind accounting `/proc/memhogs` reports.
    kind: VmoKind,
    /// Bytes this object contributed to `VMO_BYTES` at creation, subtracted
    /// verbatim on drop. Stored rather than recomputed via `self.trait_.len()`
    /// because `len()` takes the family lock — and `Drop for VmObject` can run
    /// INLINE while that very lock is already held (a last-strong-ref VMO dying
    /// inside `commit_page_internal`, whose family lock this CPU holds), so
    /// calling `len()` there re-acquires a non-reentrant ticket lock and
    /// self-deadlocks the CPU. Immutable after construction; `set_len` does not
    /// re-account `VMO_BYTES`, so this stays exactly the value that was added.
    accounted_bytes: usize,
    resizable: bool,
    /// `true` for objects with Linux `MAP_SHARED` semantics: `fork` must hand
    /// the child a mapping over the SAME object, never a copy. Set once when
    /// the mapping is established (anonymous MAP_SHARED, shared file mappings,
    /// SysV shm segments) and read by `clone_map`. An `AtomicBool` rather than
    /// a field of `inner` because `clone_map` reads it with no other reason to
    /// take the object lock.
    share_on_fork: core::sync::atomic::AtomicBool,
    trait_: Arc<dyn VMObjectTrait>,
    inner: Mutex<VmObjectInner>,
}

impl_kobject!(VmObject);
define_count_helper!(VmObject);

#[derive(Default)]
struct VmObjectInner {
    parent: Weak<VmObject>,
    children: Vec<Weak<VmObject>>,
    mapping_count: usize,
    content_size: usize,
}

/// What a VMO is backed by. Bookkeeping only -- it exists so `/proc/memhogs`
/// can say WHICH kind of object is holding physical memory when no process
/// accounts for it, which is the difference between a DRM buffer pool and an
/// orphaned file mapping and needs opposite fixes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VmoKind {
    /// Ordinary demand-zero pages.
    Paged,
    /// Demand-paged from a `FrameFiller` (a file mapping).
    PagedSource,
    /// A window onto physical memory this VMO does not own.
    Physical,
    /// Physically contiguous pages (DRM/dumb buffers, DMA).
    Contiguous,
    /// A child or slice of another VMO.
    Child,
}

const VMO_KINDS: usize = 5;
static VMO_LIVE: [AtomicUsize; VMO_KINDS] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];
static VMO_BYTES: [AtomicUsize; VMO_KINDS] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

/// Record a newly created VMO and hand its kind back for the struct field.
fn account_new(kind: VmoKind, bytes: usize) -> VmoKind {
    let i = kind_index(kind);
    VMO_LIVE[i].fetch_add(1, Ordering::Relaxed);
    VMO_BYTES[i].fetch_add(bytes, Ordering::Relaxed);
    kind
}

fn kind_index(kind: VmoKind) -> usize {
    match kind {
        VmoKind::Paged => 0,
        VmoKind::PagedSource => 1,
        VmoKind::Physical => 2,
        VmoKind::Contiguous => 3,
        VmoKind::Child => 4,
    }
}

/// Live VMO count and total declared size per kind, as
/// `[(kind, live, bytes); 5]`. Plain atomics, so it is safe to read from any
/// context -- unlike anything that walks the VMAR tree.
pub fn vmo_stats() -> [(VmoKind, usize, usize); VMO_KINDS] {
    let kinds = [
        VmoKind::Paged,
        VmoKind::PagedSource,
        VmoKind::Physical,
        VmoKind::Contiguous,
        VmoKind::Child,
    ];
    let mut out = [(VmoKind::Paged, 0, 0); VMO_KINDS];
    for (i, k) in kinds.iter().enumerate() {
        out[i] = (
            *k,
            VMO_LIVE[i].load(Ordering::Relaxed),
            VMO_BYTES[i].load(Ordering::Relaxed),
        );
    }
    out
}

impl VmObject {
    /// Create a new VMO backing on physical memory allocated in pages.
    pub fn new_paged(pages: usize) -> Arc<Self> {
        Self::new_paged_with_resizable(false, pages)
    }

    /// Create a new VMO, which can be resizable, backing on physical memory allocated in pages.
    pub fn new_paged_with_resizable(resizable: bool, pages: usize) -> Arc<Self> {
        let base = KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN);
        Arc::new(VmObject {
            resizable,
            _counter: CountHelper::new(),
            kind: account_new(VmoKind::Paged, pages * PAGE_SIZE),
            accounted_bytes: pages * PAGE_SIZE,
            share_on_fork: core::sync::atomic::AtomicBool::new(false),
            trait_: VMObjectPaged::new(pages),
            inner: Mutex::new(VmObjectInner::default()),
            base,
        })
    }

    /// Create a new paged VMO that is demand-paged from `source`.
    ///
    /// The VMO has `pages` pages; touching a page for the first time reads its
    /// contents from `source` (see [`FrameFiller`]) instead of returning zeros.
    /// Used for file-backed `mmap` so a large mapping is not read into memory up
    /// front.
    pub fn new_paged_with_source(pages: usize, source: Arc<dyn FrameFiller>) -> Arc<Self> {
        let base = KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN);
        Arc::new(VmObject {
            resizable: false,
            _counter: CountHelper::new(),
            kind: account_new(VmoKind::PagedSource, pages * PAGE_SIZE),
            accounted_bytes: pages * PAGE_SIZE,
            share_on_fork: core::sync::atomic::AtomicBool::new(false),
            trait_: VMObjectPaged::new_with_source(pages, source),
            inner: Mutex::new(VmObjectInner::default()),
            base,
        })
    }

    /// Create a paged VMO that BORROWS clean pages from `cache` (the shared
    /// per-file page cache), starting at byte `base_offset` inside it.
    ///
    /// This is the `MAP_PRIVATE` file-mapping shape: reads resolve to the
    /// cache's frames (the fault handler maps them read-only), the first write
    /// to a page copies just that page into this VMO. N processes mapping the
    /// same library therefore cost one set of frames plus their dirtied pages,
    /// not N full copies -- which is what exhausted physical memory under the
    /// desktop (three GTK processes at 266 MiB private each).
    pub fn new_paged_borrowing(
        pages: usize,
        cache: Arc<VmObject>,
        base_offset: usize,
    ) -> Arc<Self> {
        let base = KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN);
        Arc::new(VmObject {
            resizable: false,
            _counter: CountHelper::new(),
            kind: account_new(VmoKind::PagedSource, pages * PAGE_SIZE),
            accounted_bytes: pages * PAGE_SIZE,
            share_on_fork: core::sync::atomic::AtomicBool::new(false),
            trait_: VMObjectPaged::new_borrowing(pages, cache, base_offset),
            inner: Mutex::new(VmObjectInner::default()),
            base,
        })
    }

    /// Create a new VMO representing a piece of contiguous physical memory.
    pub fn new_physical(paddr: PhysAddr, pages: usize) -> Arc<Self> {
        Self::warn_if_phys_aliases_stack(paddr, pages);
        Arc::new(VmObject {
            base: KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN),
            resizable: false,
            _counter: CountHelper::new(),
            kind: account_new(VmoKind::Physical, pages * PAGE_SIZE),
            accounted_bytes: pages * PAGE_SIZE,
            share_on_fork: core::sync::atomic::AtomicBool::new(false),
            trait_: VMObjectPhysical::new(paddr, pages),
            inner: Mutex::new(VmObjectInner::default()),
        })
    }

    /// [diag] A physical VMO maps an EXISTING physical range into userspace
    /// without going through `frame_alloc`, so it escapes the allocator's
    /// live-stack alias check (`frame_alias_check`). If that range is a live
    /// coroutine stack — a nouveau GEM CPU-mmap or any other physaddr window
    /// that landed on stack memory — the userspace mapping and the kernel stack
    /// then alias the same frames, and the next write on either side is the
    /// recurring SMP null-exec ("all-zeros usable region, no guard hit", no
    /// `[frame-alias]`, no `[dma-uaf]`). Name it here, at the instant the
    /// aliasing mapping is created, before anything writes through it.
    #[cfg(not(feature = "libos"))]
    fn warn_if_phys_aliases_stack(paddr: PhysAddr, pages: usize) {
        if kernel_hal::stack_guard::paddr_aliases_stack(paddr, pages * PAGE_SIZE) {
            kernel_hal::console::serial_write_fmt_spin(format_args!(
                "\n[vmo-phys-alias] new_physical is mapping a LIVE COROUTINE STACK into \
                 userspace: paddr={:#x} pages={} — a physaddr VMO (GEM CPU-mmap / device window) \
                 landed on kernel stack memory. THIS is the SMP null-exec corruptor; symbolize \
                 the new_physical caller in the backtrace that follows this line.\n",
                paddr, pages,
            ));
        }
    }

    #[cfg(feature = "libos")]
    fn warn_if_phys_aliases_stack(_paddr: PhysAddr, _pages: usize) {}

    /// Create a VM object referring to a specific contiguous range of physical frame.
    pub fn new_contiguous(pages: usize, align_log2: usize) -> ZxResult<Arc<Self>> {
        let vmo = Arc::new(VmObject {
            base: KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN),
            resizable: false,
            _counter: CountHelper::new(),
            kind: account_new(VmoKind::Contiguous, pages * PAGE_SIZE),
            accounted_bytes: pages * PAGE_SIZE,
            share_on_fork: core::sync::atomic::AtomicBool::new(false),
            trait_: VMObjectPaged::new_contiguous(pages, align_log2)?,
            inner: Mutex::new(VmObjectInner::default()),
        });
        Ok(vmo)
    }

    /// Create a child VMO.
    pub fn create_child(
        self: &Arc<Self>,
        resizable: bool,
        offset: usize,
        len: usize,
    ) -> ZxResult<Arc<Self>> {
        let base = KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN);
        base.set_name(&self.base.name());
        let trait_ = self.trait_.create_child(offset, len)?;
        let child = Arc::new(VmObject {
            base,
            resizable,
            _counter: CountHelper::new(),
            kind: account_new(VmoKind::Child, len),
            accounted_bytes: len,
            share_on_fork: core::sync::atomic::AtomicBool::new(false),
            trait_,
            inner: Mutex::new(VmObjectInner {
                parent: Arc::downgrade(self),
                ..VmObjectInner::default()
            }),
        });
        self.add_child(&child);
        Ok(child)
    }

    /// Create a child slice as an VMO
    pub fn create_slice(self: &Arc<Self>, offset: usize, p_size: usize) -> ZxResult<Arc<Self>> {
        let size = roundup_pages(p_size);
        // why 32 * PAGE_SIZE? Refered to zircon source codes
        if size < p_size || size > usize::MAX & !(32 * PAGE_SIZE) {
            return Err(ZxError::OUT_OF_RANGE);
        }
        // child slice must be wholly contained
        let parent_size = self.trait_.len();
        if !page_aligned(offset) {
            return Err(ZxError::INVALID_ARGS);
        }
        if offset > parent_size || size > parent_size - offset {
            return Err(ZxError::INVALID_ARGS);
        }
        if self.resizable {
            return Err(ZxError::NOT_SUPPORTED);
        }
        if self.trait_.cache_policy() != CachePolicy::Cached && !self.trait_.is_contiguous() {
            return Err(ZxError::BAD_STATE);
        }
        let child = Arc::new(VmObject {
            base: KObjectBase::with(&self.base.name(), Signal::VMO_ZERO_CHILDREN),
            resizable: false,
            _counter: CountHelper::new(),
            kind: account_new(VmoKind::Child, size),
            accounted_bytes: size,
            share_on_fork: core::sync::atomic::AtomicBool::new(false),
            trait_: VMObjectSlice::new(self.trait_.clone(), offset, size),
            inner: Mutex::new(VmObjectInner {
                parent: Arc::downgrade(self),
                ..VmObjectInner::default()
            }),
        });
        self.add_child(&child);
        Ok(child)
    }

    /// Add child to the list and signal if ZeroChildren signal is active.
    /// If the number of children turns 0 to 1, signal it
    fn add_child(&self, child: &Arc<VmObject>) {
        let mut inner = self.inner.lock();
        inner.children.retain(|x| x.strong_count() != 0);
        inner.children.push(Arc::downgrade(child));
        if inner.children.len() == 1 {
            self.base.signal_clear(Signal::VMO_ZERO_CHILDREN);
        }
    }

    /// Set the length of this VMO if resizable.
    pub fn set_len(&self, len: usize) -> ZxResult {
        let size = roundup_pages(len);
        if size < len {
            return Err(ZxError::OUT_OF_RANGE);
        }
        if !self.resizable {
            return Err(ZxError::UNAVAILABLE);
        }
        self.trait_.set_len(size)
    }

    /// Set the size of the content stored in the VMO in bytes, resize vmo if needed
    pub fn set_content_size_and_resize(
        &self,
        size: usize,
        zero_until_offset: usize,
    ) -> ZxResult<usize> {
        let mut inner = self.inner.lock();
        let content_size = inner.content_size;
        let len = self.trait_.len();
        if size < content_size {
            return Ok(content_size);
        }
        let required_len = roundup_pages(size);
        let new_content_size = if required_len > len && self.set_len(required_len).is_err() {
            len
        } else {
            size
        };
        let zero_until_offset = zero_until_offset.min(new_content_size);
        if zero_until_offset > content_size {
            self.trait_
                .zero(content_size, zero_until_offset - content_size)?;
        }
        inner.content_size = new_content_size;
        Ok(new_content_size)
    }

    /// Get the size of the content stored in the VMO in bytes.
    pub fn content_size(&self) -> usize {
        let inner = self.inner.lock();
        inner.content_size
    }

    /// Get the size of the content stored in the VMO in bytes.
    pub fn set_content_size(&self, size: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        inner.content_size = size;
        Ok(())
    }

    /// Get information of this VMO.
    pub fn get_info(&self) -> VmoInfo {
        let inner = self.inner.lock();
        let mut ret = VmoInfo {
            koid: self.base.id,
            name: {
                let mut arr = [0u8; 32];
                let name = self.base.name();
                let length = name.len().min(32);
                arr[..length].copy_from_slice(&name.as_bytes()[..length]);
                arr
            },
            size: self.trait_.len() as u64,
            parent_koid: inner.parent.upgrade().map(|p| p.id()).unwrap_or(0),
            num_children: inner.children.len() as u64,
            flags: if self.resizable {
                VmoInfoFlags::RESIZABLE
            } else {
                VmoInfoFlags::empty()
            },
            cache_policy: self.trait_.cache_policy() as u32,
            share_count: inner.mapping_count as u64,
            ..Default::default()
        };
        self.trait_.complete_info(&mut ret);
        ret
    }

    /// Mark this object as Linux-`MAP_SHARED`: `fork` hands the child a
    /// mapping over this very object instead of a copy. One-way — nothing
    /// un-shares an object, matching the semantics of the flag.
    pub fn set_share_on_fork(&self) {
        self.share_on_fork
            .store(true, core::sync::atomic::Ordering::Relaxed);
    }

    /// Whether `fork` must share this object rather than copy it.
    pub fn is_share_on_fork(&self) -> bool {
        self.share_on_fork
            .load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Set the cache policy.
    pub fn set_cache_policy(&self, policy: CachePolicy) -> ZxResult {
        let inner = self.inner.lock();
        if !inner.children.is_empty() {
            return Err(ZxError::BAD_STATE);
        }
        if inner.mapping_count != 0 {
            return Err(ZxError::BAD_STATE);
        }
        self.trait_.set_cache_policy(policy)
    }

    /// Append a mapping to the VMO's mapping list.
    pub fn append_mapping(&self, mapping: Weak<VmMapping>) {
        self.inner.lock().mapping_count += 1;
        self.trait_.append_mapping(mapping);
    }

    /// Remove a mapping from the VMO's mapping list.
    pub fn remove_mapping(&self, mapping: Weak<VmMapping>) {
        self.inner.lock().mapping_count -= 1;
        self.trait_.remove_mapping(mapping);
    }

    /// Returns an estimate of the number of unique VmAspaces that this object
    /// is mapped into.
    pub fn share_count(&self) -> usize {
        let inner = self.inner.lock();
        inner.mapping_count
    }

    /// Returns true if the object size can be changed.
    pub fn is_resizable(&self) -> bool {
        self.resizable
    }

    /// Returns true if the object is backed by a contiguous range of physical memory.
    pub fn is_contiguous(&self) -> bool {
        self.trait_.is_contiguous()
    }

    /// Returns true if this is a fixed physical-memory window (see
    /// [`VMObjectTrait::is_physical`]): shared, not copied, on `fork`.
    pub fn is_physical(&self) -> bool {
        self.trait_.is_physical()
    }

    /// Returns true if this object is backed by ordinary RAM (a paged VMO).
    pub fn is_paged(&self) -> bool {
        self.trait_.is_paged()
    }

    /// Physical address of `page_idx` if already committed (never commits).
    pub fn committed_paddr(&self, page_idx: usize) -> Option<PhysAddr> {
        self.trait_.committed_paddr(page_idx)
    }

    /// Independent copy for Linux `fork`: only committed pages are copied;
    /// untouched pages stay lazy (see [`VMObjectTrait::fork_copy`]). Returns
    /// `NOT_SUPPORTED` when the backing object cannot provide those semantics,
    /// in which case the caller must do an eager full copy.
    pub fn fork_copy(self: &Arc<Self>) -> ZxResult<Arc<Self>> {
        let trait_ = self.trait_.fork_copy()?;
        let len = trait_.len();
        Ok(Arc::new(VmObject {
            base: KObjectBase::with_signal(Signal::VMO_ZERO_CHILDREN),
            resizable: false,
            _counter: CountHelper::new(),
            kind: account_new(VmoKind::Paged, len),
            accounted_bytes: len,
            share_on_fork: core::sync::atomic::AtomicBool::new(false),
            trait_,
            inner: Mutex::new(VmObjectInner {
                content_size: self.inner.lock().content_size,
                ..VmObjectInner::default()
            }),
        }))
    }
}

impl Deref for VmObject {
    type Target = Arc<dyn VMObjectTrait>;

    fn deref(&self) -> &Self::Target {
        &self.trait_
    }
}

impl Drop for VmObject {
    fn drop(&mut self) {
        // Balance the per-kind accounting first: everything below can bail out
        // through early returns and deferred drops.
        let i = kind_index(self.kind);
        VMO_LIVE[i].fetch_sub(1, Ordering::Relaxed);
        // Subtract the STORED byte count — never `self.trait_.len()`, which takes
        // the family lock. This `Drop` can run inline while that lock is already
        // held on this CPU (a last-ref VMO going out of scope inside
        // `commit_page_internal`), and `len()` there is a single-CPU self-
        // deadlock on the non-reentrant ticket lock. See `accounted_bytes`.
        VMO_BYTES[i].fetch_sub(self.accounted_bytes, Ordering::Relaxed);
        // Every `Arc<VmObject>` upgraded in here may turn out to be the LAST
        // strong reference to its object. Letting one of those go out of scope
        // inside a critical section runs its destructor INLINE, on this CPU,
        // still holding the lock — and this destructor's very next act is to
        // take the parent's lock, the one we are holding. `lock::Mutex` is a
        // non-reentrant, interrupt-disabling ticket lock with no timeout and no
        // same-CPU exemption (vendor/kernel-sync/src/ticket.rs), so that is a
        // permanent single-CPU self-deadlock: the lock is never released, every
        // later `fork` wedges behind it (it needs the same lock via
        // `share_count`/`add_child`), and each of those waiters also has
        // interrupts off. The machine stops, silently.
        //
        // The `None => continue` guard below was written for the *opposite*
        // race — a sibling already inside its own destructor, whose strong
        // count has reached zero so `upgrade` fails. That correctly kills the
        // two-CPU version. It cannot kill this one, because at `upgrade` time
        // the count is still >= 1; it reaches zero later, here, on this CPU.
        //
        // This was unreachable until copy-on-write `fork`: `fork_copy` built
        // children with `VmObjectInner::default()` (no parent), so this function
        // returned at the `None => return` below and never locked anything.
        // `create_child` is the only producer of a parented VMO, and the
        // concurrent last-reference release comes from the previous forked
        // child's address space being torn down (`Vmar::clear`) on another CPU
        // while the parent has already been reaped and forked again.
        //
        // So: park every upgraded `Arc` in `deferred` and let them die only
        // once all the guards are gone. Allocating here adds no new lock-order
        // edge — `children.append`/`retain` below already allocate under this
        // same lock.
        let mut deferred: Vec<Arc<VmObject>> = Vec::new();
        let parent = {
            let mut inner = self.inner.lock();
            let parent = match inner.parent.upgrade() {
                Some(parent) => parent,
                None => return,
            };
            for child in inner.children.iter() {
                if let Some(child) = child.upgrade() {
                    child.inner.lock().parent = Arc::downgrade(&parent);
                    deferred.push(child);
                }
            }
            let mut parent_inner = parent.inner.lock();
            let children = &mut parent_inner.children;
            children.append(&mut inner.children);
            children.retain(|c| c.strong_count() != 0);
            for child in children.iter() {
                // `retain` above filtered out dead weak refs, but on SMP another
                // CPU can drop the last strong ref between that check and this
                // `upgrade()` (concurrent process teardown of forked COW VMOs).
                // Skip the racing-away child instead of `unwrap()`-panicking.
                let child = match child.upgrade() {
                    Some(child) => child,
                    None => continue,
                };
                {
                    let mut child_inner = child.inner.lock();
                    child_inner.children.retain(|c| c.strong_count() != 0);
                    if child_inner.children.is_empty() {
                        child.base.signal_set(Signal::VMO_ZERO_CHILDREN);
                    }
                }
                deferred.push(child);
            }
            // Non-zero to zero?
            let became_childless = children.is_empty();
            drop(parent_inner);
            drop(inner);
            if became_childless {
                parent.base.signal_set(Signal::VMO_ZERO_CHILDREN);
            }
            parent
        };
        // Guards are all released; the deferred references (and our own handle
        // on the parent, which can equally be the last one) may now safely run
        // their destructors.
        drop(deferred);
        drop(parent);
    }
}

/// Describes a VMO.
#[repr(C)]
#[derive(Default)]
pub struct VmoInfo {
    /// The koid of this VMO.
    koid: KoID,
    /// The name of this VMO.
    name: [u8; 32],
    /// The size of this VMO; i.e., the amount of virtual address space it
    /// would consume if mapped.
    size: u64,
    /// If this VMO is a clone, the koid of its parent. Otherwise, zero.
    parent_koid: KoID,
    /// The number of clones of this VMO, if any.
    num_children: u64,
    /// The number of times this VMO is currently mapped into VMARs.
    num_mappings: u64,
    /// The number of unique address space we're mapped into.
    share_count: u64,
    /// Flags.
    pub flags: VmoInfoFlags,
    /// Padding.
    padding1: [u8; 4],
    /// If the type is `PAGED`, the amount of
    /// memory currently allocated to this VMO; i.e., the amount of physical
    /// memory it consumes. Undefined otherwise.
    committed_bytes: u64,
    /// If `flags & ZX_INFO_VMO_VIA_HANDLE`, the handle rights.
    /// Undefined otherwise.
    pub rights: Rights,
    /// VMO mapping cache policy.
    cache_policy: u32,
}

bitflags! {
    #[derive(Default)]
    /// Values used by ZX_INFO_PROCESS_VMOS.
    pub struct VmoInfoFlags: u32 {
        /// The VMO points to a physical address range, and does not consume memory.
        /// Typically used to access memory-mapped hardware.
        /// Mutually exclusive with TYPE_PAGED.
        const TYPE_PHYSICAL = 0;

        #[allow(clippy::identity_op)]
        /// The VMO is backed by RAM, consuming memory.
        /// Mutually exclusive with TYPE_PHYSICAL.
        const TYPE_PAGED    = 1 << 0;

        /// The VMO is resizable.
        const RESIZABLE     = 1 << 1;

        /// The VMO is a child, and is a copy-on-write clone.
        const IS_COW_CLONE  = 1 << 2;

        /// When reading a list of VMOs pointed to by a process, indicates that the
        /// process has a handle to the VMO, which isn't necessarily mapped.
        const VIA_HANDLE    = 1 << 3;

        /// When reading a list of VMOs pointed to by a process, indicates that the
        /// process maps the VMO into a VMAR, but doesn't necessarily have a handle to
        /// the VMO.
        const VIA_MAPPING   = 1 << 4;

        /// The VMO is a pager owned VMO created by zx_pager_create_vmo or is
        /// a clone of a VMO with this flag set. Will only be set on VMOs with
        /// the ZX_INFO_VMO_TYPE_PAGED flag set.
        const PAGER_BACKED  = 1 << 5;

        /// The VMO is contiguous.
        const CONTIGUOUS    = 1 << 6;
    }
}

/// Different operations that `range_change` can perform against any VmMappings that are found.
#[allow(dead_code)]
#[derive(PartialEq, Eq, Clone, Copy)]
pub(super) enum RangeChangeOp {
    Unmap,
    RemoveWrite,
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn read_write(vmo: &VmObject) {
        let mut buf = [0u8; 4];
        vmo.write(0, &[0, 1, 2, 3]).unwrap();
        vmo.read(0, &mut buf).unwrap();
        assert_eq!(&buf, &[0, 1, 2, 3]);
    }

    /// A borrower's clean page IS the cache's frame; only a written page is
    /// private. This is the mechanism that de-duplicates library mappings.
    #[test]
    fn borrower_shares_clean_pages_and_copies_on_write() {
        let cache = VmObject::new_paged(4);
        cache.write(0, &[0xAA; PAGE_SIZE]).unwrap();
        cache.write(PAGE_SIZE, &[0xBB; PAGE_SIZE]).unwrap();

        let b = VmObject::new_paged_borrowing(4, cache.clone(), 0);

        // Clean read resolves to the CACHE's frame -- same physical page, and
        // nothing gets committed in the borrower.
        let cache_paddr = cache.commit_page(0, MMUFlags::READ).unwrap();
        assert_eq!(b.commit_page(0, MMUFlags::READ).unwrap(), cache_paddr);
        assert_eq!(b.committed_pages_in_range(0, 4), 0);
        let mut buf = [0u8; 4];
        b.read(0, &mut buf).unwrap();
        assert_eq!(buf, [0xAA; 4]);

        // Write faults copy up: the borrower now owns a DIFFERENT frame for
        // page 0, with the cache's content as its starting point.
        let priv_paddr = b.commit_page(0, MMUFlags::WRITE).unwrap();
        assert_ne!(priv_paddr, cache_paddr);
        assert_eq!(b.committed_pages_in_range(0, 4), 1);
        b.write(0, &[0x11; 8]).unwrap();
        b.read(0, &mut buf).unwrap();
        assert_eq!(buf, [0x11; 4]);
        let mut tail = [0u8; 4];
        b.read(8, &mut tail).unwrap();
        assert_eq!(tail, [0xAA; 4], "copy-up must start from the cache content");

        // The cache is untouched by the borrower's write...
        cache.read(0, &mut buf).unwrap();
        assert_eq!(buf, [0xAA; 4]);
        // ...and page 1 is still borrowed, so a write INTO the cache is
        // visible through the borrower's clean page (Linux page-cache
        // semantics for unmodified MAP_PRIVATE pages).
        cache.write(PAGE_SIZE, &[0xCC; 4]).unwrap();
        b.read(PAGE_SIZE, &mut buf).unwrap();
        assert_eq!(buf, [0xCC; 4]);
    }

    /// A borrower window starting mid-cache reads the right pages.
    #[test]
    fn borrower_honours_base_offset() {
        let cache = VmObject::new_paged(4);
        cache.write(2 * PAGE_SIZE, &[0x77; 8]).unwrap();
        let b = VmObject::new_paged_borrowing(2, cache, 2 * PAGE_SIZE);
        let mut buf = [0u8; 8];
        b.read(0, &mut buf).unwrap();
        assert_eq!(buf, [0x77; 8]);
    }

    /// Pages beyond the cache window are ordinary demand-zero private pages
    /// (the BSS tail of a mapping), and writing them never touches the cache.
    #[test]
    fn borrower_tail_beyond_cache_is_demand_zero() {
        let cache = VmObject::new_paged(1);
        cache.write(0, &[0xAA; 4]).unwrap();
        let b = VmObject::new_paged_borrowing(3, cache.clone(), 0);
        // Assert demand-zero by IDENTITY (the shared zero frame), not by
        // content: `physical::tests::read_write` maps raw paddr 0x1000 and
        // writes through it, and in the libos mock allocator that address can
        // be the lazily-allocated global ZERO_FRAME itself -- a pre-existing
        // test-environment collision (`zero_page_write` sits ignored for the
        // same reason) that made a content check order-dependent.
        assert_eq!(
            b.commit_page(1, MMUFlags::READ).unwrap(),
            kernel_hal::mem::ZERO_FRAME.paddr(),
        );
        let mut buf = [0u8; 4];
        b.write(PAGE_SIZE, &[0x55; 4]).unwrap();
        b.read(PAGE_SIZE, &mut buf).unwrap();
        assert_eq!(buf, [0x55; 4]);
        assert_eq!(cache.committed_pages_in_range(0, 1), 1);
    }

    /// fork keeps the borrow: the child's untouched pages still read the
    /// cache, its copied pages stay private.
    #[test]
    fn borrower_fork_copy_keeps_borrowing() {
        let cache = VmObject::new_paged(2);
        cache.write(0, &[0xAA; 4]).unwrap();
        cache.write(PAGE_SIZE, &[0xBB; 4]).unwrap();
        let b = VmObject::new_paged_borrowing(2, cache.clone(), 0);
        b.write(0, &[0x11; 4]).unwrap(); // dirty page 0

        let child = b.fork_copy().unwrap();
        let mut buf = [0u8; 4];
        child.read(0, &mut buf).unwrap();
        assert_eq!(buf, [0x11; 4], "dirtied page must be copied to the child");
        child.read(PAGE_SIZE, &mut buf).unwrap();
        assert_eq!(buf, [0xBB; 4], "clean page must still borrow from the cache");
        // Writes in the child stay in the child.
        child.write(PAGE_SIZE, &[0x22; 4]).unwrap();
        b.read(PAGE_SIZE, &mut buf).unwrap();
        assert_eq!(buf, [0xBB; 4]);
    }

    /// The hidden-node snapshot tree must refuse borrowers: a hidden parent
    /// knows nothing of the cache and would resolve clean pages to zeros.
    #[test]
    fn borrower_refuses_create_child() {
        let cache = VmObject::new_paged(1);
        let b = VmObject::new_paged_borrowing(1, cache, 0);
        assert!(b.create_child(false, 0, PAGE_SIZE).is_err());
    }
}

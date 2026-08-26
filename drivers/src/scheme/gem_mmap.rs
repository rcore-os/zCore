//! Physical-address registry for driver-private GEM objects that need to be
//! CPU-mmap-able through the same "fake mmap offset" mechanism
//! `linux-object` already uses for `CREATE_DUMB` buffers (`DrmDev::get_vmo`
//! in `linux-object/src/fs/devfs/drm_scheme.rs`, decoding `offset >>
//! PAGE_SHIFT` back into a handle id and looking it up). That mechanism
//! only knows about `linux-object`'s own generic GEM handle table
//! (`drm::get_handle`, populated by `CREATE_DUMB`/PRIME import) -- a lower
//! crate like this one cannot register into it (`drivers` has no
//! dependency on `linux-object`; see `display::nouveau_uapi`'s module doc
//! for the layering constraint this works around). This is the same
//! problem [`crate::scheme::syncobj`] solves for DRM syncobjs: state that
//! both a driver (`nvidia.rs`'s nouveau-uAPI `GEM_NEW`, populating it) and
//! `linux-object`'s ioctl dispatch (`get_vmo`, querying it) need to reach,
//! living in the lower crate both already depend on.
//!
//! # Handle namespace
//!
//! Entries here are looked up by the SAME id space `get_vmo` decodes from
//! `offset >> PAGE_SHIFT` for `linux-object`'s own handle table, so a
//! collision between the two tables would resolve to the wrong physical
//! range -- silently mapping the wrong memory into a process. This module
//! does not enforce disjointness itself (it has no visibility into
//! `linux-object`'s counter); callers must. Convention: `linux-object`'s
//! `DRM_STATE.next_handle_id` starts at 1 and grows sequentially, so
//! driver-private handles registered here use the high half of the `u32`
//! range instead (see `nouveau_uapi.rs`'s `nouveau_gem_next_handle`
//! starting at `0x8000_0001`). Collision is only possible after billions
//! of `CREATE_DUMB` allocations in a single boot, not a real constraint.

use alloc::vec::Vec;
use lock::Mutex;

struct MappedGem {
    handle: u32,
    phys_addr: u64,
    size: u64,
    /// PRIME share count. A driver-private GEM object can be referenced by
    /// more than one holder at once: the original `GEM_NEW` owner (the
    /// compositor's GBM/NVK allocation) plus every PRIME *self-import* of the
    /// same buffer (`FD_TO_HANDLE` handing back this ORIGINAL handle, see
    /// `lookup_by_phys`). Real Linux DRM keeps the GEM object alive as long as
    /// any handle or dma-buf references it; we model that with this count.
    /// `register` starts it at 1 (the creator), `add_ref` bumps it on each
    /// self-import, `dec_ref` drops it on each `GEM_CLOSE`, and the object is
    /// only truly freed when it reaches 0. Without this, NVK importing then
    /// closing a swapchain buffer that wlroots still owns tore the mapping out
    /// from under wlroots, so the next self-import missed and fell back to a
    /// generic handle -> `GEM_INFO` ENOENT -> zink "couldn't allocate memory".
    refcount: u32,
}

/// Outcome of [`dec_ref`], so `GEM_CLOSE` can tell "still shared, keep it" from
/// "last reference gone, free it" from "not one of ours".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecRef {
    /// The reference was dropped but others remain; `refcount` is the new
    /// value (> 0). The GEM object and its backing memory MUST stay alive.
    StillReferenced(u32),
    /// The last reference was dropped; the entry has already been removed from
    /// this table. The caller should now free the GEM object / RM memory.
    Freed,
    /// `handle` was never registered here (a GEM object with no phys mapping,
    /// or an unknown handle). The caller decides what that means.
    NotTracked,
}

lazy_static::lazy_static! {
    static ref MAPPINGS: Mutex<Vec<MappedGem>> = Mutex::new(Vec::new());
}

/// Registers the physical mapping for `handle`, starting its PRIME share count
/// at 1. If `handle` is already present (re-registration of the same id) only
/// the physical range is updated -- the existing share count is preserved so a
/// stray re-register can never resurrect a freed refcount.
pub fn register(handle: u32, phys_addr: u64, size: u64) {
    let mut table = MAPPINGS.lock();
    if let Some(e) = table.iter_mut().find(|e| e.handle == handle) {
        e.phys_addr = phys_addr;
        e.size = size;
    } else {
        table.push(MappedGem {
            handle,
            phys_addr,
            size,
            refcount: 1,
        });
    }
}

/// Adds a PRIME reference to `handle` (a self-import resolved back to this
/// original nouveau handle). Returns the new share count, or `None` if the
/// handle is not tracked here. Pairs with [`dec_ref`] on `GEM_CLOSE`.
pub fn add_ref(handle: u32) -> Option<u32> {
    let mut table = MAPPINGS.lock();
    table.iter_mut().find(|e| e.handle == handle).map(|e| {
        e.refcount = e.refcount.saturating_add(1);
        e.refcount
    })
}

/// Drops one PRIME reference to `handle` (a `GEM_CLOSE`). See [`DecRef`] for the
/// three outcomes. When the count reaches 0 the entry is removed here BEFORE the
/// caller frees the backing memory, preserving the "no mmap-able mapping can
/// outlive the VRAM it points at" invariant `GEM_CLOSE` has always kept.
pub fn dec_ref(handle: u32) -> DecRef {
    let mut table = MAPPINGS.lock();
    let Some(pos) = table.iter().position(|e| e.handle == handle) else {
        return DecRef::NotTracked;
    };
    let e = &mut table[pos];
    if e.refcount > 1 {
        e.refcount -= 1;
        DecRef::StillReferenced(e.refcount)
    } else {
        table.remove(pos);
        DecRef::Freed
    }
}

/// Drops `handle`'s mapping unconditionally, ignoring the share count. Returns
/// whether one existed. This is the forced-teardown path (process exit): the
/// owning context is gone, so every reference it held dies with it regardless
/// of count. Runtime `GEM_CLOSE` must use [`dec_ref`], not this.
pub fn unregister(handle: u32) -> bool {
    let mut table = MAPPINGS.lock();
    if let Some(pos) = table.iter().position(|e| e.handle == handle) {
        table.remove(pos);
        true
    } else {
        false
    }
}

/// Looks up `handle`'s `(phys_addr, size)`, e.g. from `DrmDev::get_vmo`.
pub fn lookup(handle: u32) -> Option<(u64, u64)> {
    MAPPINGS
        .lock()
        .iter()
        .find(|e| e.handle == handle)
        .map(|e| (e.phys_addr, e.size))
}

/// Reverse lookup: which driver-private GEM handle owns the object whose
/// backing starts at `phys_addr`? Used by PRIME `FD_TO_HANDLE`: importing a
/// dma-buf that THIS driver exported must hand back the ORIGINAL nouveau
/// handle (real Linux DRM semantics -- self-import resolves to the existing
/// GEM object), because only that handle works with the nouveau-uAPI
/// `GEM_INFO`/`VM_BIND`/`EXEC`. A fresh generic handle over the same memory
/// looks fine to the generic ioctls and then fails every driver-private one.
pub fn lookup_by_phys(phys_addr: u64) -> Option<(u32, u64)> {
    MAPPINGS
        .lock()
        .iter()
        .find(|e| e.phys_addr == phys_addr)
        .map(|e| (e.handle, e.size))
}

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
}

lazy_static::lazy_static! {
    static ref MAPPINGS: Mutex<Vec<MappedGem>> = Mutex::new(Vec::new());
}

/// Registers (or replaces) the physical mapping for `handle`.
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
        });
    }
}

/// Drops `handle`'s mapping (e.g. on `GEM_CLOSE`). Returns whether one
/// existed -- callers use this to decide whether `handle` was theirs at
/// all, not just whether it happened to have a CPU mapping registered.
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

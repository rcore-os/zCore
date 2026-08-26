# Design: file-backed `mmap` writeback, `msync(2)`, and a shared page cache

Status: **design only — not implemented**. This document is the plan for work
that should be done against a *building, runnable* kernel, because it mutates
user memory and the filesystem; a mistake corrupts user data, so it must not be
landed blind.

It builds on the demand-paging readahead already added in
`linux-object/src/fs/file.rs` (`Readahead` / `FileFrameFiller`).

---

## 1. Current state and the gap

### How a file `mmap` works today

`sys_mmap` (`linux-syscall/src/vm.rs`) for a file-backed mapping:

1. Calls `file_like.get_vmo(offset, len)`.
2. `File::get_vmo` (`linux-object/src/fs/file.rs`) builds a **fresh**
   `VmObject::new_paged_with_source(pages(len), FileFrameFiller{..})` — a paged
   VMO demand-paged from the inode via `FrameFiller::fill_page`.
3. Maps that VMO into the process VMAR with `map_ext(.., map_range=false)`, so
   pages are read from the file on first touch.

Key consequences:

- **No write-back.** Nothing ever copies modified pages back to the inode.
- **`MAP_SHARED` is not honoured.** `MmapFlags::SHARED` vs `PRIVATE` is never
  inspected; every mapping gets its own private VMO. Writes to a `MAP_SHARED`
  region are **silently lost** on `munmap`/exit.
- **No coherence.** Two `mmap`s of the same inode get two independent VMOs, so
  they do not see each other's in-memory writes, and `read(2)`/`write(2)` on the
  fd is incoherent with the mapping.
- **`msync(2)` does not exist.** It is not in the syscall dispatch
  (`linux-syscall/src/lib.rs`); a caller gets the unknown-syscall path.

### What the kernel already gives us to build on

- `VMObjectTrait` (`zircon-object/src/vm/vmo/mod.rs`) exposes `read(offset,
  buf)` / `write(offset, buf)` and `committed_pages_in_range(start, end)`.
- `VMObjectPaged` keeps committed frames in `frames: BTreeMap<page_idx,
  PageState>`. **There is no dirty bit** — `PageState` only carries a
  `PageStateTag` (`Owned`/…), not write state.
- `LinuxProcessInner` (`linux-object/src/process.rs`) already hangs per-process
  VM bookkeeping (`brk`, `mapped_brk`); it is the natural home for a mapping
  registry. **There is currently no list of file-backed mappings.**

---

## 2. Scope decision: staged, not "full Linux page cache"

A fully coherent page cache (one shared set of pages per inode, backing
`read`/`write`/all `mmap`s, with write-back) is the end goal but a deep VM
change. Propose **three stages**, each independently shippable and each strictly
better than today:

- **Stage A — single-mapping `MAP_SHARED` write-back + `msync(2)`.** Persist a
  shared writable file mapping's changes on `msync` and `munmap`/exit. *No*
  cross-mapping coherence yet. Covers the common case (a process mmaps a file,
  writes, persists) and removes the silent-data-loss bug.
- **Stage B — per-inode shared VMO cache.** `get_vmo` returns the *same* VMO for
  the same inode+range, so all `MAP_SHARED` mappings share pages and are mutually
  coherent. Write-back becomes per-inode.
- **Stage C — unify `read`/`write` with the page cache.** Route
  `File::read_at`/`write_at` through the shared VMO so fd I/O and mmap are fully
  coherent. Largest change; do last.

This document specifies **Stage A** in full and sketches B/C.

---

## 3. Stage A — `MAP_SHARED` write-back + `msync(2)`

### 3.1 Dirty tracking

Two options; pick per how much the VM layer can expose safely:

1. **Committed-page write-back (simplest, correct, wasteful).** On flush, write
   back every *committed* page in the mapped range. Reading an uncommitted
   source page would just re-fault the file's own bytes, so writing back only
   *committed* pages is sufficient — and writing back a committed-but-unmodified
   page rewrites identical bytes (a no-op on disk content). Needs a way to
   enumerate committed page indices (add `VMObjectTrait::for_each_committed_page`
   or reuse `committed_pages_in_range` plus an index iterator).
   - Cost: rewrites clean-but-touched pages. Fine for a first cut; note it.

2. **Hardware dirty-bit harvest (proper).** Add a real `dirty` flag to
   `PageState`, set it from the PTE dirty bit (`MMUFlags`/`GenericPageTable`)
   when harvesting, and write back only dirty pages, clearing the bit. This is
   how Linux does it and is the right Stage-B/C foundation, but touches the
   arch page-table code — do it once a build exists to validate the PTE plumbing
   per arch (x86_64/aarch64/riscv64).

**Recommendation:** ship Stage A with option 1 (purely in object/syscall code,
no arch changes), then upgrade to option 2 in Stage B.

### 3.2 The mapping registry

Add to `LinuxProcessInner`:

```rust
/// File-backed shared writable mappings, for msync/munmap write-back.
/// Keyed by mapping start address.
shared_file_maps: BTreeMap<usize, SharedFileMap>,
```

```rust
struct SharedFileMap {
    vmo: Arc<VmObject>,     // the mapping's VMO (already held by the VMAR)
    inode: Arc<dyn INode>,  // write-back target
    file_offset: usize,     // inode offset that VMO offset 0 maps to
    len: usize,             // mapping length in bytes
    source_len: usize,      // writable/in-file span (== FileFrameFiller.source_len)
}
```

`File::get_vmo` already computes `source_len`; return it (or the
`FileFrameFiller`) so `sys_mmap` can populate the registry. Provide
`LinuxProcess` accessors mirroring the existing `brk`/`mapped_brk` ones:
`record_shared_file_map`, `take_shared_file_maps_in(addr, len)`,
`shared_file_maps_overlapping(addr, len)`.

### 3.3 `sys_mmap` changes

In the file-backed branch of `sys_mmap`, after a successful `map_ext`:

```rust
if flags.contains(MmapFlags::SHARED) && prot.contains(MmapProt::WRITE) {
    self.linux_process().record_shared_file_map(addr, SharedFileMap {
        vmo: vmo.clone(), inode, file_offset: offset as usize, len, source_len,
    });
}
```

`MAP_PRIVATE` writable file mappings must **not** be registered (their writes
are copy-on-write and never hit the file). NB: zCore does not yet COW such
private mappings — out of scope here, but worth a follow-up, since today a
private writable file mapping shares the source VMO.

### 3.4 Write-back routine

```rust
/// Copy the mapping's committed pages back to its inode. Idempotent.
fn writeback(map: &SharedFileMap) -> LxResult {
    let mut buf = [0u8; PAGE_SIZE];
    let end = map.source_len;                 // never write past the in-file span
    for page_start in (0..end).step_by(PAGE_SIZE) {
        let idx = page_start / PAGE_SIZE;
        if !map.vmo.is_committed(idx) { continue; }   // skip never-touched pages
        let n = (end - page_start).min(PAGE_SIZE);
        map.vmo.read(page_start, &mut buf[..n])?;     // VMO -> buf
        map.inode.write_at(map.file_offset + page_start, &buf[..n])?;
    }
    Ok(())
}
```

- Clamp to `source_len` so the BSS tail past EOF is never written.
- `is_committed(idx)` is the new accessor (option 1 above); with option 2 it
  becomes `is_dirty(idx)` and the bit is cleared after write.
- Errors: collect the first error but continue, returning it at the end, so one
  bad block does not silently skip the rest (matches Linux `msync` best-effort).

### 3.5 `msync(2)`

Add `Sys::MSYNC => self.sys_msync(a0, a1, a2)` to the dispatch and:

```rust
pub fn sys_msync(&self, addr: usize, len: usize, flags: usize) -> SysResult {
    // flags: MS_ASYNC=1, MS_INVALIDATE=2, MS_SYNC=4. MS_ASYNC|MS_SYNC is EINVAL.
    if addr % PAGE_SIZE != 0 { return Err(LxError::EINVAL); }
    if flags & !(MS_ASYNC|MS_SYNC|MS_INVALIDATE) != 0 { return Err(LxError::EINVAL); }
    if (flags & MS_ASYNC != 0) && (flags & MS_SYNC != 0) { return Err(LxError::EINVAL); }
    for map in self.linux_process().shared_file_maps_overlapping(addr, len) {
        writeback(&map)?;       // MS_ASYNC may defer; with no writeback queue, treat as sync
    }
    Ok(0)
}
```

Since there is no async write-back thread, `MS_ASYNC` is honoured by writing
synchronously (allowed — it is "schedule write-back", and doing it now is a
correct superset).

### 3.6 `munmap` / exit

In `sys_munmap`, before `vmar.unmap`, flush and drop any overlapping shared file
maps:

```rust
for map in self.linux_process().take_shared_file_maps_in(addr, len) {
    let _ = writeback(&map);   // best-effort on unmap, like Linux
}
```

On process teardown, flush all remaining registered maps. Find the existing
exit/`vmar` teardown path and add the sweep there.

### 3.7 Edge cases

- **Partial unmap** of a registered region: split/trim the registry entry (only
  flush+forget the overlapping sub-range; keep the remainder). Simplest first
  cut: flush the whole entry and re-register the surviving sub-range.
- **`mprotect` dropping `PROT_WRITE`**: optionally flush then deregister; safe to
  leave registered (write-back stays correct, just possibly redundant).
- **File truncated after mmap**: `write_at` past current size either extends or
  errors depending on the FS; clamp to `source_len` captured at mmap and accept
  FS semantics. Document it.
- **`fork`**: a `MAP_SHARED` mapping stays shared with the child. The registry
  must be inherited (the VMAR fork already shares the VMO); copy entries to the
  child in the fork path.

---

## 4. Stage B — per-inode shared VMO cache (coherence)

- Cache `Weak<VmObject>` per `(inode-id, file_offset, len)` (or per inode with a
  single full-file VMO and child VMOs for sub-ranges via
  `VMObject::create_child`).
- `File::get_vmo` returns the cached VMO instead of a fresh one, so all
  `MAP_SHARED` mappings of the same inode share pages → mutual coherence.
- Write-back becomes per-inode (flush the inode's VMO), driven by the same
  `msync`/`munmap` hooks; the registry keys on the inode.
- Requires lifetime care: the cache holds weak refs so a closed file's VMO is
  reclaimed; the last `munmap` flushes.
- Best paired with the **hardware dirty-bit** tracking (§3.1 option 2) so a
  large shared cache does not rewrite clean pages.

## 5. Stage C — unify `read`/`write` with the cache

- Route `File::read_at`/`write_at` through the inode's cached VMO
  (`vmo.read`/`vmo.write`) rather than straight to `inode.read_at`/`write_at`,
  with the VMO as the single source of truth and a write-back path to the inode.
- Gives full `read`/`write` ↔ `mmap` coherence (POSIX).
- Largest blast radius (every fd read/write); do last, behind tests, with the
  dirty-bit machinery from Stage B.

---

## 6. Test strategy

Pure/unit-testable now (no kernel boot):

- **Write-back range math**: page stepping, `source_len` clamp, partial last
  page, skip-uncommitted — mirror the `Readahead` equivalence-test approach with
  a mock inode (a `Vec<u8>`) and a mock "committed pages" set; assert the bytes
  written equal the pages modified and nothing past `source_len` is touched.
- **`msync` flag validation**: `MS_SYNC|MS_ASYNC` → EINVAL, unknown bits →
  EINVAL, unaligned addr → EINVAL (pure, like the `madvise_advice_known` tests).
- **Registry overlap/split**: `shared_file_maps_overlapping` / `take_..._in` and
  the partial-unmap split logic as pure `BTreeMap` operations.

Requires a running kernel (the reason this is design-only):

- `MAP_SHARED` write + `msync` + re-`read` sees the change; `MAP_PRIVATE` write
  does **not** hit the file; two mappings coherent (Stage B); `read`/`mmap`
  coherent (Stage C); multi-arch PTE dirty-bit harvest (Stage B option 2).

---

## 7. Risk summary

| Change | Blast radius | Failure mode if wrong |
|---|---|---|
| Registry + `msync` + munmap flush (Stage A) | syscalls + process state | wrong/lost write-back → **user file corruption** |
| Committed-page enumeration accessor | VMO object code | over/under-writeback (wasteful or missed data) |
| PTE dirty-bit harvest (Stage B) | arch page tables ×3 | missed dirty page → lost writes |
| Per-inode VMO cache (Stage B) | mmap path | aliasing / use-after-free of shared pages |
| `read`/`write` through cache (Stage C) | every fd I/O | data-path regressions |

Every row mutates user data, so each stage must be landed against a building
kernel with the runtime tests in §6 green. That is why this is written down
rather than committed as code.

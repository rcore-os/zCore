use super::*;
use alloc::vec::Vec;
use bitflags::bitflags;
use zircon_object::vm::{pages, roundup_pages, MMUFlags, VmObject, PAGE_SIZE};

/// Per-call cap for a single `mmap` / `brk` growth. It bounds how much a single
/// syscall can commit at once (physical frames + per-page VMO metadata).
///
/// This must be larger than the biggest single-call reservation we expect from
/// userspace. Two very different consumers push this up:
///   * the dynamic linker maps a library's whole LOAD span in one call, and
///     `libLLVM.so` (pulled in by `perf`) is ~150 MiB — the original 128 MiB cap
///     rejected it outright with ENOMEM (surfaced by musl as "Out of memory")
///     despite gigabytes being free;
///   * SpiderMonkey (Firefox's JS engine) reserves its whole JIT executable
///     region up front as a single `PROT_NONE` anonymous `mmap`. On x86-64 that
///     reservation is ~1.1 GiB (`MaxCodeBytesPerProcess`), so a 1 GiB cap
///     bounced it — and, crucially, the early-return did so *silently*, which is
///     why `js::jit::InitProcessExecutableMemory() failed` fired with no mmap
///     error in the log. 2 GiB clears the reservation with headroom.
///
/// Because anonymous mappings are now demand-paged (see `sys_mmap`), a large
/// reservation costs only address space plus a sparse per-touched-page frame
/// entry, not committed RAM — so the cap bounds address-space requests, not
/// physical footprint.
const MAX_MMAP_LEN: usize = 2 * 1024 * 1024 * 1024;

/// Linux `vm.mmap_min_addr` (default 64 KiB): floor for kernel-chosen mmap
/// placement. Without it the VMAR's first-fit search can hand a non-FIXED
/// `mmap(NULL, ...)` the address 0 — userspace then treats 0 as a valid
/// pointer and every syscall null-check bounces it with EFAULT (observed:
/// `dd bs=4096` failing "Bad address" because glibc placed its I/O buffer
/// there). Also restores NULL-dereference protection for user processes.
/// MAP_FIXED requests are exempt, matching Linux for privileged tooling.
const MMAP_MIN_ADDR: usize = 0x1_0000;

/// Syscalls for virtual memory.
///
/// # Menu
///
/// - [`mmap`](Self::sys_mmap)
/// - [`mprotect`](Self::sys_mprotect)
/// - [`munmap`](Self::sys_munmap)
/// Assert, at the point `mmap` is about to hand `addr` to userspace, that the
/// range it promises is actually installed in the VMAR.
///
/// Userspace is faulting with `[vmar-near] NO mapping within 0x20000 bytes --
/// the address was never mapped` at PAGE-ALIGNED addresses, inside musl
/// mallocng's `m->mem->meta = m` — the allocator writing the FIRST word of a
/// group it just got back from mmap. So either mmap returned a range it never
/// installed, or the range was installed and something removed it afterwards.
/// Those two need completely different fixes, and this check separates them:
/// it fires HERE only in the first case, and stays silent in the second (the
/// crash then proves the range was destroyed later).
///
/// Only the first and last page are probed, so the cost is two lookups, not one
/// per page — and the first page is exactly the word mallocng writes.
fn verify_installed(
    vmar: &alloc::sync::Arc<zircon_object::vm::VmAddressRegion>,
    addr: usize,
    len: usize,
    kind: &str,
) {
    let last = addr + len - PAGE_SIZE;
    let first_ok = vmar.find_mapping(addr).is_some();
    let last_ok = vmar.find_mapping(last).is_some();
    if !first_ok || !last_ok {
        kernel_hal::klog_info!(
            "[mmap-lost] {} mmap returned {:#x} len={:#x} but the range is NOT installed \
             (first_page={}, last_page={})",
            kind,
            addr,
            len,
            if first_ok { "ok" } else { "MISSING" },
            if last_ok { "ok" } else { "MISSING" },
        );
    }
}

impl Syscall<'_> {
    /// Map files or devices into memory
    /// (see [linux man mmap(2)](https://www.man7.org/linux/man-pages/man2/mmap.2.html)).
    ///
    /// `sys_mmap` creates a new mapping in the virtual address space of the calling process.
    ///
    /// The starting address for the new mapping is specified in `addr`.
    ///
    /// The `len` argument specifies the length of the mapping (which must be greater than 0).
    ///
    /// Arguments `fd` and `offset` specifies mapping file descriptor and offset in the file.
    ///
    /// The `prot` argument describes the desired memory protection of the mapping
    /// (and must not conflict with the open mode of the file).
    /// It is either 0 or the bitwise OR of one or more of the following flags:
    ///
    /// - **`MmapProt::READ`**
    ///
    ///   Pages may be read
    ///
    /// - **`MmapProt::WRITE`**
    ///
    ///   Pages may be written
    ///
    /// - **`MmapProt::EXEC`**
    ///
    ///   Pages may be executed
    ///
    /// The `flags` argument determines whether updates to the mapping are visible to other processes mapping the same region,
    /// and whether updates are carried through to the underlying file.
    /// This behavior is determined by including exactly one of the following values:
    ///
    /// - **`MmapFlags::SHARED`**
    ///
    ///   Share this mapping. Updates to the mapping are visible to other processes mapping the same region,
    ///   and (in the case of file-backed mappings) are carried through to the underlying file.
    ///   (To precisely control when updates are carried through to the underlying file requires the use of `msync`,
    ///   which has not been implemented in zcore).
    ///
    /// - **`MmapFlags::PRIVATE`**
    ///
    ///   Create a private copy-on-write mapping.
    ///   Updates to the mapping are not visible to other processes mapping the same file,
    ///   and are not carried through to the underlying file.
    ///   It is unspecified whether changes made to the file after the `sys_mmap` call are visible in the mapped region.
    ///
    /// - **`MmapFlags::FIXED`**
    ///
    ///   Don't interpret `addr` as a hint: place the mapping at exactly that address.
    ///   `addr` must be suitably aligned:
    ///   for most architectures a multiple of the page size is sufficient;
    ///   however, some architectures may impose additional restrictions.
    ///   If the memory region specified by `addr` and `len` overlaps pages of any existing mapping(s),
    ///   then the overlapped part of the existing mapping(s) will be discarded.
    ///   If the specified address cannot be used, `sys_mmap` will fail.
    ///
    /// - **`MmapFlags::ANONYMOUS`**
    ///
    ///   The mapping is not backed by any file; its contents are initialized to zero.
    ///   Both `fd` and `offset` arguments are ignored.
    ///   The use of `MmapFlags::ANONYMOUS` in conjunction with `MmapFlags::SHARED`
    ///   causes an [`EINVAL`](LxError::EINVAL) to be returned.
    pub async fn sys_mmap(
        &self,
        addr: usize,
        len: usize,
        prot: usize,
        flags: usize,
        fd: FileDesc,
        offset: u64,
    ) -> SysResult {
        let prot = MmapProt::from_bits_truncate(prot);
        let flags = MmapFlags::from_bits_truncate(flags);
        info!(
            "mmap: addr={:#x}, size={:#x}, prot={:?}, flags={:?}, fd={:?}, offset={:#x}",
            addr, len, prot, flags, fd, offset
        );
        if len == 0 || len > MAX_MMAP_LEN {
            // Log oversized requests: the early-return is the one mmap failure
            // path with no other trace, and a silently-rejected giant
            // reservation (e.g. a JIT engine's executable pool) otherwise looks
            // like a spontaneous userspace crash with nothing in the kernel log.
            warn!(
                "mmap: rejecting len={:#x} (cap={:#x}) prot={:?} flags={:?} fd={:?}",
                len, MAX_MMAP_LEN, prot, flags, fd
            );
            return Err(LxError::ENOMEM);
        }
        // Linux UAPI: `len` is rounded UP to whole pages by the kernel for
        // mmap/munmap/mprotect; only `addr` must be page-aligned (and only
        // for MAP_FIXED). glibc's ld.so depends on this — its zero-fill
        // mmap passes the raw unaligned segment length, and our former
        // aligned-length requirement bounced it with EINVAL, killing every
        // glibc binary at load with "cannot map zero-fill pages" (musl
        // rounds in userspace, which hid the gap for years).
        if flags.contains(MmapFlags::FIXED) && !addr.is_multiple_of(PAGE_SIZE) {
            return Err(LxError::EINVAL);
        }
        let len = roundup_pages(len);
        // hunter W^X: reject (or audit) simultaneously writable+executable maps.
        if !hunter::check_mmap(
            self.zircon_process().id(),
            prot.contains(MmapProt::WRITE),
            prot.contains(MmapProt::EXEC),
        ) {
            return Err(LxError::EACCES);
        }

        let proc = self.zircon_process();
        let pid = proc.id();
        let vmar = proc.vmar();
        let want_write = prot.contains(MmapProt::WRITE);

        // mmap_lock (see `LinuxProcess::aspace_lock`): every layout mutation
        // below runs under it, so a concurrent `fork` can never clone a
        // half-updated mapping list. Taken before any `inner` access (fd
        // lookups in the file-backed arm) — that is the global lock order.
        let _aspace = self.linux_process().aspace_lock().lock();
        let fixed = flags.contains(MmapFlags::FIXED);
        if fixed {
            // hunter: the range is about to be replaced, so drop its W^X
            // writable-history. (The unmap itself now happens INSIDE
            // map_ext_min -- see `overwrite` below.)
            hunter::check_munmap(pid, addr, len);
        }
        // MAP_FIXED must be ATOMIC. This used to call vmar.unmap() here, before
        // trying to place the new mapping: the range was destroyed first, and
        // if the mapping then failed for any reason the process was simply left
        // without the memory it had -- mmap returned an error, userspace kept
        // its own view of the old allocation, and the next touch took a SIGSEGV
        // on an address with no mapping at all. Caught live: xfce4-session
        // faulting at 0x4db2000 with
        //   [vmar-unmapped-by] removed by mmap_fixed_preunmap of [0x4db2000, 0x4f7c000)
        //   [vmar-near] NO mapping within 0x20000 bytes -- never mapped
        // i.e. the pre-unmap ran and nothing replaced it. Linux never does
        // this: on MAP_FIXED failure the old mapping stays untouched.
        //
        // `map_ext_min`'s `overwrite` does the same removal under the SAME VMAR
        // lock, immediately before inserting the replacement, so the range is
        // never left destroyed-and-empty.
        let vmar_offset = fixed.then(|| addr - vmar.addr());
        if flags.contains(MmapFlags::ANONYMOUS) {
            let vmo = VmObject::new_paged(pages(len));
            // MAP_SHARED | MAP_ANONYMOUS: the region must be one object shared
            // with every future child, not a per-process copy. Without the
            // marker, fork privatized it and preforked pools (nginx, postgres,
            // any parent-child counter page) silently stopped sharing.
            if flags.contains(MmapFlags::SHARED) {
                vmo.set_share_on_fork();
            }
            // Demand-page anonymous memory (`map_range = false`) instead of
            // committing a zero frame for every page up front. Linux mmap does
            // not commit anonymous pages until first touch, and some programs
            // rely on that: SpiderMonkey reserves ~1.1 GiB of `PROT_NONE` JIT
            // address space in a single call and only ever touches the slices
            // it turns into code — eagerly committing the whole reservation
            // would burn >1 GiB of RAM (and, before the cap was raised, OOM).
            // The permission ceiling is RXW so a later `mprotect` can raise a
            // slice to executable (JIT), matching the file-mapping path. Both
            // user- and kernel-mode faults on these pages resolve through
            // `Vmar::handle_page_fault`, so a lazily-mapped buffer handed to a
            // syscall (e.g. `read`) still faults in correctly on the kernel
            // store.
            let addr = vmar
                .map_ext_min(
                    vmar_offset,
                    vmo.clone(),
                    0,
                    vmo.len(),
                    MMUFlags::RXW | MMUFlags::USER,
                    prot.to_flags(),
                    fixed,
                    false,
                    MMAP_MIN_ADDR,
                )
                .inspect_err(|e| {
                    warn!(
                        "mmap(anon) FAILED: {:?} addr={:#x} len={:#x} prot={:?} flags={:?}",
                        e, addr, len, prot, flags
                    );
                })?;
            // hunter P3: remember a writable mapping so a later mprotect(EXEC)
            // over it is recognised as the two-step W^X bypass.
            hunter::record_mapping(pid, addr, len, want_write);
            verify_installed(&vmar, addr, len, "anon");
            Ok(addr)
        } else {
            let file_like = self.linux_process().get_file_like(fd)?;
            // MAP_SHARED must hand every mapper of the file the SAME VmObject
            // (stores propagate between processes — the wl_shm pixel path);
            // MAP_PRIVATE keeps the per-call demand-paged snapshot.
            let (vmo, vmo_offset) = if flags.contains(MmapFlags::SHARED) {
                let (vmo, off) = file_like
                    .get_vmo_shared(offset as usize, len)
                    .inspect_err(|e| {
                        warn!(
                            "mmap(file,shared) get_vmo_shared FAILED: {:?} fd={:?} offset={:#x} len={:#x}",
                            e, fd, offset, len
                        );
                    })?;
                // Same rule as anonymous MAP_SHARED: fork must share, not copy.
                vmo.set_share_on_fork();
                (vmo, off)
            } else {
                let vmo = file_like.get_vmo(offset as usize, len).inspect_err(|e| {
                    warn!(
                        "mmap(file) get_vmo FAILED: {:?} fd={:?} offset={:#x} len={:#x}",
                        e, fd, offset, len
                    );
                })?;
                (vmo, 0)
            };
            // Permission ceiling = full RXW: Linux lets mprotect raise a file
            // mapping to any R/W/X combination, and the dynamic linker relies on
            // it — ld.so mprotect()s a library's *executable* text segment to RW
            // to apply text relocations / DT_TEXTREL and to set up GNU_RELRO
            // (seen with libxul). A W^X-preserving ceiling that strips WRITE from
            // exec maps made that mprotect fail; the syscall layer then swallowed
            // the error, leaving GOT entries unrelocated (== 0) and Firefox
            // crashing on a store through a NULL pointer. W^X is still audited by
            // hunter::check_mprotect (Report mode by default) — it just isn't
            // enforced by capping the mapping here.
            let ceiling = MMUFlags::RXW | MMUFlags::USER;
            // Map without committing the range up front (`map_range = false`):
            // the VMO returned by `get_vmo` is demand-paged from the file, so its
            // pages are read in on the faults that first touch them. Eagerly
            // mapping the whole range here would defeat that and re-introduce the
            // full-file read that froze the machine on `perf` (libLLVM ~150 MiB).
            // For a shared full-file VMO the requested window starts at
            // `vmo_offset` inside it; snapshots bake the offset in and use 0.
            let map_len = len.min(vmo.len() - vmo_offset);
            let addr = if map_len < len {
                // The file cannot back the whole request (mmap past EOF, or a
                // PT_LOAD segment whose memsz exceeds its filesz). Linux STILL
                // maps the full `len`: the bytes past the file read as zero and
                // the region exists for its entire length. Mapping only
                // `map_len` -- which is what this did -- handed userspace a
                // region that ENDS EARLY. Nothing reports an error: mmap
                // returns success and the caller's own length, so the allocator
                // or loader happily uses the tail, and the first touch past the
                // truncation point takes a SIGSEGV with NOT_FOUND (no VmMapping
                // covers it). Observed as musl mallocng writing through the end
                // of a group, and every XFCE component dying that way.
                //
                // Reserve the full range with an anonymous VMO first, then
                // overwrite its head with the file window, so the tail is
                // demand-zero exactly like Linux.
                let anon = VmObject::new_paged(pages(len));
                let base = vmar
                    .map_ext_min(
                        vmar_offset,
                        anon,
                        0,
                        len,
                        ceiling,
                        prot.to_flags(),
                        fixed,
                        false,
                        MMAP_MIN_ADDR,
                    )
                    .inspect_err(|e| {
                        warn!(
                            "mmap(file) reserve FAILED: {:?} len={:#x} map_len={:#x}",
                            e, len, map_len
                        );
                    })?;
                vmar.map_ext_min(
                    Some(base - vmar.addr()),
                    vmo.clone(),
                    vmo_offset,
                    map_len,
                    ceiling,
                    prot.to_flags(),
                    true,
                    false,
                    MMAP_MIN_ADDR,
                )
                .inspect_err(|e| {
                    warn!(
                        "mmap(file) overlay FAILED: {:?} base={:#x} map_len={:#x} vmo_len={:#x}",
                        e,
                        base,
                        map_len,
                        vmo.len()
                    );
                })?;
                base
            } else {
                vmar.map_ext_min(
                    vmar_offset,
                    vmo.clone(),
                    vmo_offset,
                    map_len,
                    ceiling,
                    prot.to_flags(),
                    fixed,
                    false,
                    MMAP_MIN_ADDR,
                )
                .inspect_err(|e| {
                    warn!(
                        "mmap(file) map_ext FAILED: {:?} addr={:#x} len={:#x} vmo_len={:#x} prot={:?} flags={:?} offset={:#x}",
                        e, addr, len, vmo.len(), prot, flags, offset
                    );
                })?
            };
            hunter::record_mapping(pid, addr, len, want_write);
            verify_installed(&vmar, addr, len, "file");
            Ok(addr)
        }
    }

    /// Change the location of the program break
    /// (see [linux man brk(2)](https://www.man7.org/linux/man-pages/man2/brk.2.html)).
    ///
    /// `sys_brk` sets the end of the process data segment (the program break) to `new_brk`.
    /// If `new_brk` is 0, the current break is returned unchanged (query mode).
    /// If `new_brk` is below the current break, the heap is shrunk and the freed pages are
    /// unmapped.  If `new_brk` is above the current break, new anonymous pages are mapped and
    /// the new break value is returned.
    /// On failure the current break is returned (Linux semantics: never returns -1).
    ///
    /// The initial program break is set during ELF loading (end of all loaded segments) and
    /// stored on the process via `LinuxProcess::set_brk`.  A value of 0 means not yet
    /// initialized (e.g. the process called `brk` before any ELF was loaded).
    pub fn sys_brk(&self, new_brk: usize) -> SysResult {
        // mmap_lock: brk grows/shrinks the heap mapping — a layout mutation.
        let _aspace = self.linux_process().aspace_lock().lock();
        // Reserve heap pages in 1 MiB chunks instead of issuing one VMO per
        // user-visible call. Glibc malloc typically calls `brk` in 4–32 KiB
        // increments, which the old code translated into a fresh VMO + VMAR
        // mapping each time — hundreds of VMAR entries for a single arena
        // and a corresponding bump in allocator / TLB-shootdown pressure.
        const BRK_CHUNK: usize = 1 << 20;

        let proc = self.linux_process();
        let current_brk = proc.brk();
        // Lazy init: the loader populates `brk` directly before the first
        // sys_brk, so the mapped end matches it.
        let mapped_brk = {
            let m = proc.mapped_brk();
            if m == 0 {
                current_brk
            } else {
                m
            }
        };
        info!(
            "brk: new_brk={:#x}, current_brk={:#x}, mapped_brk={:#x}",
            new_brk, current_brk, mapped_brk
        );

        // brk(0) → return current break unchanged (query).
        if new_brk == 0 {
            return Ok(current_brk);
        }

        let new_brk_aligned = roundup_pages(new_brk);
        let vmar = self.zircon_process().vmar();

        if new_brk_aligned < current_brk {
            // Shrink: just move the user-visible break. The reserved pages
            // stay mapped until they are reused on the next grow. Linux glibc
            // essentially never shrinks brk, and skipping the unmap avoids
            // the VMAR churn + TLB shootdown for transient shrink/grow
            // patterns.
            proc.set_brk(new_brk_aligned);
            info!("brk: shrunk to {:#x} (mapping kept)", new_brk_aligned);
            Ok(new_brk_aligned)
        } else if new_brk_aligned > current_brk {
            // Inside the already-reserved heap region — bookkeeping only.
            if new_brk_aligned <= mapped_brk {
                proc.set_brk(new_brk_aligned);
                info!(
                    "brk: extended to {:#x} (within reserved mapping)",
                    new_brk_aligned
                );
                return Ok(new_brk_aligned);
            }
            // Extend the mapping. Round the request up to BRK_CHUNK so a
            // burst of small grows is satisfied by a single map_at.
            let want = new_brk_aligned - mapped_brk;
            let size = want
                .checked_next_multiple_of(BRK_CHUNK)
                .unwrap_or(want)
                .max(BRK_CHUNK);
            if size > MAX_MMAP_LEN {
                return Ok(current_brk);
            }
            let new_mapped_brk = mapped_brk + size;
            let vmo = VmObject::new_paged(pages(size));
            let flags = MMUFlags::READ | MMUFlags::WRITE | MMUFlags::USER;
            // vmar.addr() == 0 for user address spaces, so VMAR offset == absolute VA.
            match vmar.map_at(mapped_brk, vmo, 0, size, flags) {
                Ok(_) => {
                    proc.set_brk(new_brk_aligned);
                    proc.set_mapped_brk(new_mapped_brk);
                    info!(
                        "brk: extended to {:#x}, mapping reserved up to {:#x}",
                        new_brk_aligned, new_mapped_brk
                    );
                    Ok(new_brk_aligned)
                }
                Err(e) => {
                    warn!(
                        "brk: failed to map {:#x} bytes at {:#x}: {:?}",
                        size, mapped_brk, e
                    );
                    // Return current break on failure (Linux semantics).
                    Ok(current_brk)
                }
            }
        } else {
            // Already at requested break (after rounding).
            Ok(current_brk)
        }
    }

    /// Set protection on a region of memory
    /// (see [linux man mprotect(2)](https://www.man7.org/linux/man-pages/man2/mprotect.2.html)).
    ///
    /// `sys_mprotect` changes the access protections for the calling process's memory pages
    /// containing any part of the address range in the interval `[addr, addr+len-1]`.
    /// `addr` must be aligned to a page boundary.
    ///
    /// If the calling process tries to access memory in a manner that violates the protections,
    /// then the kernel generates a SIGSEGV signal for the process.
    ///
    /// `prot` is a combination of the following access flags:
    /// 0 or a bitwise-or of the other values in the following list:
    ///
    /// - **`MmapProt::READ`**
    ///
    ///   The memory can be read.
    ///
    /// - **`MmapProt::WRITE`**
    ///
    ///   The memory can be modified.
    ///
    /// - **`MmapProt::EXEC`**
    ///
    ///   The memory can be executed.
    ///
    /// If `prot` is 0, the memory cannot be accessed at all.
    pub fn sys_mprotect(&self, addr: usize, len: usize, prot: usize) -> SysResult {
        // mmap_lock: mprotect SPLITS mappings (`cut`) — the exact mutation
        // that, racing a fork's copy loop, gave the child an address space
        // that never existed (llvmpipe's W^X churn vs the Xwayland fork).
        let _aspace = self.linux_process().aspace_lock().lock();
        let prot = MmapProt::from_bits_truncate(prot);
        info!(
            "mprotect: addr={:#x}, size={:#x}, prot={:?}",
            addr, len, prot
        );
        // Linux UAPI: addr must be page-aligned; len is rounded up to pages.
        if !addr.is_multiple_of(PAGE_SIZE) {
            return Err(LxError::EINVAL);
        }
        let len = roundup_pages(len);
        // hunter W^X: reject (or audit) transitions to writable+executable,
        // including the two-step mmap(W)-then-mprotect(X) bypass (it tracks the
        // ever-writable history of this exact range).
        let pid = self.zircon_process().id();
        if !hunter::check_mprotect(
            pid,
            addr,
            len,
            prot.contains(MmapProt::WRITE),
            prot.contains(MmapProt::EXEC),
        ) {
            return Err(LxError::EACCES);
        }
        let proc = self.zircon_process();
        let vmar = proc.vmar();
        let flags = prot.to_flags();
        // Attempt the real permission change. Normally a range that overlaps
        // sub-regions (Zircon's protect() restriction) is treated as a benign
        // no-op. But under W^X *enforcement* a failed *narrowing* that leaves
        // pages more permissive than requested would be a silent bypass, so we
        // surface the error instead of swallowing it.
        match vmar.protect(addr, len, flags) {
            Ok(()) => Ok(0),
            Err(e) => {
                if hunter::policy::wx_mode() == hunter::Mode::Enforce {
                    warn!(
                        "mprotect: addr={:#x} len={:#x} flags={:?} → {:?} (rejected under W^X enforce)",
                        addr, len, flags, e
                    );
                    return Err(LxError::EINVAL);
                }
                warn!(
                    "mprotect: addr={:#x} len={:#x} flags={:?} → {:?} (ignored)",
                    addr, len, flags, e
                );
                Ok(0)
            }
        }
    }

    /// Unmap files or devices into memory
    /// (see [linux man munmap(2)](https://www.man7.org/linux/man-pages/man2/munmap.2.html)).
    ///
    /// Deletes the mappings for the specified address range, and causes further references to addresses
    /// within the range to generate invalid memory references.
    ///
    /// The `sys_munmap` system call deletes the mappings for the specified address range,
    /// and causes further references to addresses within the range to generate invalid memory references.
    /// The region is also automatically unmapped when the process is terminated.
    /// On the other hand, closing the file descriptor does not unmap the region.
    ///
    /// Both `addr` and `len` must be aligned to the page size, additionally, `len` must greater than 0.
    /// Otherwise, an [`EINVAL`](LxError::EINVAL) is returned.
    pub fn sys_munmap(&self, addr: usize, len: usize) -> SysResult {
        // mmap_lock: removal is a layout mutation (see sys_mmap).
        let _aspace = self.linux_process().aspace_lock().lock();
        info!("munmap: addr={:#x}, size={:#x}", addr, len);
        // Linux UAPI: addr must be page-aligned; len is rounded up to pages.
        if !addr.is_multiple_of(PAGE_SIZE) || len == 0 {
            return Err(LxError::EINVAL);
        }
        let len = roundup_pages(len);
        let proc = self.thread.proc();
        // hunter P3: the range is gone, so drop its W^X writable-history.
        hunter::check_munmap(proc.id(), addr, len);
        let vmar = proc.vmar();
        // Tagged so the fault dump can tell a real userspace munmap apart
        // from MAP_FIXED's pre-unmap. Two giant unmaps were seen destroying
        // live memory: [0x23cb000,0x4f7e000) (~44 MiB) and
        // [0x23e3000,0x3c3e000) (~24 MiB), the second starting exactly at the
        // address mallocng had just been handed by mmap.
        vmar.unmap_why(addr, len, "sys_munmap")?;
        Ok(0)
    }

    /// Remap a virtual memory address
    /// (see [linux man mremap(2)](https://www.man7.org/linux/man-pages/man2/mremap.2.html)).
    ///
    /// Expands or shrinks the mapping containing `[old_addr, old_addr+old_len)`,
    /// moving it when the space after it is taken (only if `MREMAP_MAYMOVE` is
    /// set, else `ENOMEM`) or to the exact address in `new_addr` with
    /// `MREMAP_FIXED`. Contents are preserved: the kernel re-maps the same
    /// backing object at the new range, and pages added by growth read back as
    /// zero. This is the allocator fast-path (musl and glibc `realloc` try
    /// mremap first for large chunks), so answering it for real — instead of the
    /// former unconditional `ENOMEM` — saves a malloc+memcpy+free per grow.
    ///
    /// `MREMAP_DONTUNMAP` (Linux ≥ 5.7) is not supported and returns `EINVAL`,
    /// which is exactly what a pre-5.7 kernel says.
    pub fn sys_mremap(
        &self,
        old_addr: usize,
        old_len: usize,
        new_len: usize,
        flags: usize,
        new_addr: usize,
    ) -> SysResult {
        const MREMAP_MAYMOVE: usize = 1;
        const MREMAP_FIXED: usize = 2;
        const MREMAP_DONTUNMAP: usize = 4;
        // mmap_lock: remap moves/resizes mappings (see sys_mmap).
        let _aspace = self.linux_process().aspace_lock().lock();
        info!(
            "mremap: old_addr={:#x}, old_len={:#x}, new_len={:#x}, flags={:#x}, new_addr={:#x}",
            old_addr, old_len, new_len, flags, new_addr
        );
        if flags & !(MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP) != 0 {
            return Err(LxError::EINVAL);
        }
        if flags & MREMAP_DONTUNMAP != 0 {
            return Err(LxError::EINVAL);
        }
        if flags & MREMAP_FIXED != 0 && flags & MREMAP_MAYMOVE == 0 {
            return Err(LxError::EINVAL);
        }
        // Linux: old_addr must be page-aligned; lengths round up to pages. A
        // zero old_len (the MAP_SHARED duplication trick) is not supported.
        if !old_addr.is_multiple_of(PAGE_SIZE) || old_len == 0 || new_len == 0 {
            return Err(LxError::EINVAL);
        }
        let old_len = roundup_pages(old_len);
        let new_len = roundup_pages(new_len);
        if new_len > MAX_MMAP_LEN {
            return Err(LxError::ENOMEM);
        }

        let proc = self.zircon_process();
        let pid = proc.id();
        let vmar = proc.vmar();
        // Whether the range was ever writable, for hunter's W^X history on the
        // range the pages land in. Read before the move invalidates old_addr.
        let writable = vmar
            .find_mapping(old_addr)
            .and_then(|m| m.get_flags(old_addr).ok())
            .map(|f| f.contains(MMUFlags::WRITE))
            .unwrap_or(true);
        let fixed = (flags & MREMAP_FIXED != 0).then_some(new_addr);
        let ret = vmar
            .remap(
                old_addr,
                old_len,
                new_len,
                flags & MREMAP_MAYMOVE != 0,
                fixed,
            )
            .map_err(|e| match e {
                zircon_object::ZxError::NOT_FOUND => LxError::EFAULT,
                zircon_object::ZxError::INVALID_ARGS => LxError::EINVAL,
                _ => LxError::ENOMEM,
            })?;
        if ret == old_addr {
            // Resized in place: retire the dropped tail from hunter's history or
            // record the grown one, depending on the direction.
            if new_len < old_len {
                hunter::check_munmap(pid, old_addr + new_len, old_len - new_len);
            } else if new_len > old_len {
                hunter::record_mapping(pid, old_addr + old_len, new_len - old_len, writable);
            }
        } else {
            hunter::check_munmap(pid, old_addr, old_len);
            hunter::record_mapping(pid, ret, new_len, writable);
        }
        Ok(ret)
    }

    /// Synchronize a file with a memory map
    /// (see [linux man msync(2)](https://www.man7.org/linux/man-pages/man2/msync.2.html)).
    ///
    /// Argument checking follows the man page exactly (`EINVAL` for a misaligned
    /// address, unknown flags or `MS_SYNC|MS_ASYNC` together; `ENOMEM` when the
    /// range touches unmapped pages). No writeback happens beyond that: shared
    /// file mappings hand every mapper the same `VmObject` as the file cache, so
    /// stores are already visible to readers the way `msync` is meant to
    /// guarantee — succeeding here is honest, and it un-breaks software that
    /// treats an `msync` failure as fatal (sqlite, mandb).
    pub fn sys_msync(&self, addr: usize, len: usize, flags: usize) -> SysResult {
        const MS_ASYNC: usize = 1;
        const MS_INVALIDATE: usize = 2;
        const MS_SYNC: usize = 4;
        info!(
            "msync: addr={:#x}, len={:#x}, flags={:#x}",
            addr, len, flags
        );
        if flags & !(MS_ASYNC | MS_INVALIDATE | MS_SYNC) != 0
            || (flags & MS_SYNC != 0 && flags & MS_ASYNC != 0)
            || !addr.is_multiple_of(PAGE_SIZE)
        {
            return Err(LxError::EINVAL);
        }
        let vmar = self.zircon_process().vmar();
        let mut page = addr;
        let end = addr + roundup_pages(len);
        while page < end {
            if vmar.find_mapping(page).is_none() {
                return Err(LxError::ENOMEM);
            }
            page += PAGE_SIZE;
        }
        Ok(0)
    }

    /// Determine whether pages are resident in memory
    /// (see [linux man mincore(2)](https://www.man7.org/linux/man-pages/man2/mincore.2.html)).
    ///
    /// Writes one byte per page into `vec`; bit 0 set means the page is resident.
    /// Residency is answered from the page table: a demand-paged page that has
    /// never been touched has no PTE and reports non-resident, exactly the
    /// distinction Linux draws. `ENOMEM` when the range includes unmapped pages,
    /// which some allocators use to probe address-space layout.
    pub fn sys_mincore(&self, addr: usize, len: usize, mut vec: UserOutPtr<u8>) -> SysResult {
        info!("mincore: addr={:#x}, len={:#x}", addr, len);
        if !addr.is_multiple_of(PAGE_SIZE) {
            return Err(LxError::EINVAL);
        }
        let pages_count = pages(len);
        let vmar = self.zircon_process().vmar();
        let mut residency = Vec::with_capacity(pages_count);
        for i in 0..pages_count {
            let page = addr + i * PAGE_SIZE;
            let mapping = vmar.find_mapping(page).ok_or(LxError::ENOMEM)?;
            let resident = mapping.query_vaddr(page).is_ok();
            residency.push(resident as u8);
        }
        vec.write_array(&residency)?;
        Ok(0)
    }

    /// Every page of `[addr, addr+len)` (addr rounded down, len rounded up,
    /// as mlock(2) specifies) must belong to a mapping, else `ENOMEM` — the
    /// address-range validation `mlock`/`munlock` owe their callers.
    fn check_locked_range(&self, addr: usize, len: usize) -> SysResult {
        let start = addr / PAGE_SIZE * PAGE_SIZE;
        // A hostile `len` can wrap the end computation; a wrapped range cannot
        // be fully mapped, so ENOMEM is the right answer for it too. The walk
        // itself is bounded: it stops at the first hole, so it never scans
        // beyond the actually-mapped span plus one page.
        let end = addr
            .checked_add(len)
            .map(roundup_pages)
            .ok_or(LxError::ENOMEM)?;
        let vmar = self.zircon_process().vmar();
        let mut page = start;
        while page < end {
            vmar.find_mapping(page).ok_or(LxError::ENOMEM)?;
            page += PAGE_SIZE;
        }
        Ok(0)
    }

    /// Lock part of the calling process's memory into RAM (see mlock(2) and
    /// Documentation/mm/unevictable-lru.rst).
    ///
    /// This kernel has no swap and never evicts anonymous pages, so every
    /// mapped page is already permanently resident — after validating the
    /// range the way Linux does, "locked" is the truthful answer. This is what
    /// key-handling software (gpg, ssh-agent) needs from mlock: the guarantee
    /// that secrets never hit backing store.
    pub fn sys_mlock(&self, addr: usize, len: usize) -> SysResult {
        info!("mlock: addr={:#x}, len={:#x}", addr, len);
        if len == 0 {
            return Ok(0);
        }
        self.check_locked_range(addr, len)
    }

    /// `mlock2` = mlock plus a `flags` word; the only defined flag is
    /// `MLOCK_ONFAULT` (see mlock2(2)).
    pub fn sys_mlock2(&self, addr: usize, len: usize, flags: usize) -> SysResult {
        info!(
            "mlock2: addr={:#x}, len={:#x}, flags={:#x}",
            addr, len, flags
        );
        const MLOCK_ONFAULT: usize = 1;
        if flags & !MLOCK_ONFAULT != 0 {
            return Err(LxError::EINVAL);
        }
        self.sys_mlock(addr, len)
    }

    /// Unlock previously locked pages (see mlock(2)); range rules match
    /// `sys_mlock`.
    pub fn sys_munlock(&self, addr: usize, len: usize) -> SysResult {
        info!("munlock: addr={:#x}, len={:#x}", addr, len);
        if len == 0 {
            return Ok(0);
        }
        self.check_locked_range(addr, len)
    }

    /// Lock the whole address space (see mlockall(2)). Flags are validated
    /// exactly as Linux does; with residency permanent here, success follows.
    pub fn sys_mlockall(&self, flags: usize) -> SysResult {
        info!("mlockall: flags={:#x}", flags);
        check_mlockall_flags(flags)
    }

    /// Undo `mlockall` (see munlockall(2)).
    pub fn sys_munlockall(&self) -> SysResult {
        info!("munlockall");
        Ok(0)
    }

    /// Give advice about use of memory
    /// (see [linux man madvise(2)](https://www.man7.org/linux/man-pages/man2/madvise.2.html)).
    ///
    /// `madvise` is advisory: the kernel is free to ignore the hint. zCore does
    /// not yet act on any advice, so a recognised request succeeds without
    /// changing the mapping. An unrecognised advice value, or a start address
    /// that is not page-aligned, is rejected with `EINVAL` as on Linux — which
    /// is stricter (and more correct) than the previous stub that accepted any
    /// value, including garbage, with success.
    pub fn sys_madvise(&self, addr: usize, len: usize, advice: usize) -> SysResult {
        info!(
            "madvise: addr={:#x}, len={:#x}, advice={}",
            addr, len, advice
        );
        if !madvise_advice_known(advice) {
            return Err(LxError::EINVAL);
        }
        if !addr.is_multiple_of(PAGE_SIZE) {
            return Err(LxError::EINVAL);
        }
        // MADV_DONTNEED (4) / MADV_FREE (8) must actually DISCARD the pages:
        // Linux guarantees the range reads back as zero on next access. Memory
        // allocators rely on this — they decommit a region with MADV_DONTNEED
        // and later REUSE it assuming it is zeroed. Treating it as a pure no-op
        // leaves STALE data, which corrupts allocator metadata on reuse:
        //   * apk's mimalloc aborts ("trying to free from an invalid arena")
        //     mid-`apk update`, killing every package operation;
        //   * Firefox's mozjemalloc red-black tree corrupts (the earlier crash).
        // We DISCARD the range for real (see `Vmar::madv_dontneed`): unmap the
        // PTEs and drop the VMO frames, so the next touch faults in a fresh zero
        // page. An earlier version only zeroed page-table-visible pages, which
        // silently skipped a page whose frame the VMO still held but whose PTE
        // had been dropped/PROT_NONE'd — mimalloc reused exactly such a page and
        // found its old bytes, hence the deterministic "corrupted free list
        // entry" abort during `apk update`.
        const MADV_DONTNEED: usize = 4;
        const MADV_FREE: usize = 8;
        if (advice == MADV_DONTNEED || advice == MADV_FREE) && len != 0 {
            // Zero the committed pages in the range THROUGH THE VMO so they read
            // back as zero on next access — including pages whose PTE was dropped
            // or turned PROT_NONE, which a page-table walk cannot see (that gap
            // let mimalloc reuse a stale frame and abort). Done in place, leaving
            // the mapping and frames intact (an earlier unmap+decommit variant
            // turned the abort into a SIGSEGV).
            let proc = self.zircon_process();
            let vmar = proc.vmar();
            vmar.madv_dontneed(addr, roundup_pages(len));
        }
        Ok(0)
    }
}

/// `madvise(2)` advice values zCore recognises. All of zCore's target arches
/// (x86_64/aarch64/riscv64) share this (asm-generic) numbering:
///   0 NORMAL, 1 RANDOM, 2 SEQUENTIAL, 3 WILLNEED, 4 DONTNEED, 8 FREE,
///   9 REMOVE, 10 DONTFORK, 11 DOFORK, 12 MERGEABLE, 13 UNMERGEABLE,
///   14 HUGEPAGE, 15 NOHUGEPAGE, 16 DONTDUMP, 17 DODUMP, 18 WIPEONFORK,
///   19 KEEPONFORK, 20 COLD, 21 PAGEOUT, 100 HWPOISON, 101 SOFT_OFFLINE.
const KNOWN_MADVISE: &[usize] = &[
    0, 1, 2, 3, 4, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 100, 101,
];

/// Whether `advice` is a `madvise(2)` value zCore recognises. Pure, so the
/// classification is unit-testable independently of the syscall plumbing.
fn madvise_advice_known(advice: usize) -> bool {
    KNOWN_MADVISE.contains(&advice)
}

#[cfg(test)]
mod madvise_tests {
    use super::madvise_advice_known;

    #[test]
    fn known_advice_is_accepted() {
        // NORMAL/RANDOM/SEQUENTIAL/WILLNEED/DONTNEED/FREE and a few higher ones.
        for a in [0usize, 1, 2, 3, 4, 8, 19, 21, 100, 101] {
            assert!(madvise_advice_known(a), "advice {} should be known", a);
        }
    }

    #[test]
    fn unknown_advice_is_rejected() {
        // Gaps in the numbering (5,6,7) and out-of-range values are unknown.
        for a in [5usize, 6, 7, 22, 99, 102, usize::MAX] {
            assert!(!madvise_advice_known(a), "advice {} should be unknown", a);
        }
    }
}

bitflags! {
    /// for the flag argument in mmap()
    pub struct MmapFlags: usize {
        #[allow(clippy::identity_op)]
        /// Changes are shared.
        const SHARED = 1 << 0;
        /// Changes are private.
        const PRIVATE = 1 << 1;
        /// Place the mapping at the exact address
        const FIXED = 1 << 4;
        /// The mapping is not backed by any file. (non-POSIX)
        const ANONYMOUS = MMAP_ANONYMOUS;
    }
}

/// MmapFlags `MMAP_ANONYMOUS` depends on arch
#[cfg(target_arch = "mips")]
const MMAP_ANONYMOUS: usize = 0x800;
#[cfg(not(target_arch = "mips"))]
const MMAP_ANONYMOUS: usize = 1 << 5;

bitflags! {
    /// for the prot argument in mmap()
    pub struct MmapProt: usize {
        #[allow(clippy::identity_op)]
        /// Data can be read
        const READ = 1 << 0;
        /// Data can be written
        const WRITE = 1 << 1;
        /// Data can be executed
        const EXEC = 1 << 2;
    }
}

impl MmapProt {
    /// convert MmapProt to MMUFlags
    fn to_flags(self) -> MMUFlags {
        let mut flags = MMUFlags::USER;
        if self.contains(MmapProt::READ) {
            flags |= MMUFlags::READ;
        }
        if self.contains(MmapProt::WRITE) {
            flags |= MMUFlags::WRITE;
        }
        if self.contains(MmapProt::EXEC) {
            flags |= MMUFlags::EXECUTE;
        }
        // FIXME: hack for unimplemented mprotect
        if self.is_empty() {
            flags |= MMUFlags::READ | MMUFlags::WRITE;
        }
        flags
    }
}

/// mlockall(2) `MCL_CURRENT`: lock all currently mapped pages.
const MCL_CURRENT: usize = 1;
/// mlockall(2) `MCL_FUTURE`: lock everything mapped from now on.
const MCL_FUTURE: usize = 2;
/// mlockall(2) `MCL_ONFAULT`: lock pages as they are faulted in.
const MCL_ONFAULT: usize = 4;

/// Validate an mlockall(2) `flags` word. Pure, so the matrix is unit-testable:
/// empty or unknown flags are `EINVAL`, and `MCL_ONFAULT` is only meaningful
/// alongside `MCL_CURRENT`/`MCL_FUTURE` — the exact rules from the man page.
fn check_mlockall_flags(flags: usize) -> SysResult {
    if flags == 0 || flags & !(MCL_CURRENT | MCL_FUTURE | MCL_ONFAULT) != 0 {
        return Err(LxError::EINVAL);
    }
    if flags == MCL_ONFAULT {
        return Err(LxError::EINVAL);
    }
    Ok(0)
}

#[cfg(test)]
mod mlockall_flag_tests {
    use super::*;

    #[test]
    fn valid_combinations_are_accepted() {
        for flags in [
            MCL_CURRENT,
            MCL_FUTURE,
            MCL_CURRENT | MCL_FUTURE,
            MCL_CURRENT | MCL_ONFAULT,
            MCL_FUTURE | MCL_ONFAULT,
            MCL_CURRENT | MCL_FUTURE | MCL_ONFAULT,
        ] {
            assert_eq!(check_mlockall_flags(flags), Ok(0), "{flags:#x}");
        }
    }

    #[test]
    fn empty_lone_onfault_and_unknown_bits_are_einval() {
        for flags in [0, MCL_ONFAULT, 8, MCL_CURRENT | 16, usize::MAX] {
            assert_eq!(
                check_mlockall_flags(flags),
                Err(LxError::EINVAL),
                "{flags:#x}"
            );
        }
    }
}

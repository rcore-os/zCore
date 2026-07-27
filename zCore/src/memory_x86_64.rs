//! Physical frame allocation and kernel heap on x86_64 bare metal.
//!
//! Two pools on purpose:
//! - **Kernel heap** (`GlobalAlloc`): fixed BSS buddy pool for `Vec`/strings/smoltcp.
//! - **Frame allocator** (bitmap): UEFI free RAM for DMA pages and process VM.
//!
//! Do not `transfer()` raw UEFI regions into the kernel heap at early boot: the buddy
//! allocator touches those pages and can hang before the kernel reaches 60% progress.

use bitmap_allocator::BitAlloc;
use core::ops::Range;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use kernel_hal::PhysAddr;
use lock::Mutex;

static TOTAL_MEMORY: AtomicUsize = AtomicUsize::new(0);
static HEAP_USED: AtomicUsize = AtomicUsize::new(0);
static FRAMES_USED: AtomicUsize = AtomicUsize::new(0);

type FrameAlloc = bitmap_allocator::BitAlloc16M; // max 64G

const PAGE_BITS: usize = 12;
const MAX_MANAGED_PADDR_EXCLUSIVE: PhysAddr = 1usize << (PAGE_BITS + 24); // 64GiB

static FRAME_ALLOCATOR: Mutex<FrameAlloc> = Mutex::new(FrameAlloc::DEFAULT);

// ─── DEBUG: detector de doble-uso / use-after-free de frames físicos ───────────
//
// Bitset "frame actualmente asignado", paralelo al bitmap allocator. Cubre hasta
// 4 GiB (suficiente para el bench de 1 GiB). Empieza todo a 0 (libre) porque el
// allocator solo reparte frames libres y solo marcamos al repartir.
//   - DOUBLE ALLOC: el allocator devuelve un frame que aún teníamos marcado como
//     asignado -> dos dueños del mismo frame físico (la causa de la corrupción).
//   - DOUBLE/UNTRACKED FREE: se libera un frame que no estaba asignado ->
//     liberación prematura / doble free.
const TRACK_MAX_FRAMES: usize = 1 << 20; // 4 GiB / 4 KiB
const TRACK_WORDS: usize = TRACK_MAX_FRAMES / 64;
static FRAME_ALLOCATED: [AtomicU64; TRACK_WORDS] = [const { AtomicU64::new(0) }; TRACK_WORDS];

fn track_mark_alloc(idx: usize) {
    if idx >= TRACK_MAX_FRAMES {
        return;
    }
    let bit = 1u64 << (idx % 64);
    let prev = FRAME_ALLOCATED[idx / 64].fetch_or(bit, Ordering::SeqCst);
    if prev & bit != 0 {
        crate::klog_warn!(
            "[frametrack] DOUBLE ALLOC frame_idx={:#x} paddr={:#x} (dos dueños del mismo frame)",
            idx,
            idx << PAGE_BITS
        );
    }
}

fn track_mark_free(idx: usize) {
    if idx >= TRACK_MAX_FRAMES {
        return;
    }
    let bit = 1u64 << (idx % 64);
    let prev = FRAME_ALLOCATED[idx / 64].fetch_and(!bit, Ordering::SeqCst);
    if prev & bit == 0 {
        crate::klog_warn!(
            "[frametrack] DOUBLE/UNTRACKED FREE frame_idx={:#x} paddr={:#x} (liberación prematura)",
            idx,
            idx << PAGE_BITS
        );
    }
}

#[inline]
fn phys_addr_to_frame_idx(addr: PhysAddr) -> usize {
    addr >> PAGE_BITS
}

#[inline]
fn frame_idx_to_phys_addr(idx: usize) -> PhysAddr {
    idx << PAGE_BITS
}

pub fn insert_regions(regions: &[Range<PhysAddr>]) {
    debug!("init_frame_allocator regions: {regions:x?}");
    let mut ba = FRAME_ALLOCATOR.lock();
    for region in regions {
        let mut start = region.start.min(MAX_MANAGED_PADDR_EXCLUSIVE);
        let end = region.end.min(MAX_MANAGED_PADDR_EXCLUSIVE);
        if start < 0x1000 {
            start = 0x1000;
        }
        if end <= start {
            continue;
        }
        if end != region.end {
            crate::klog_warn!(
                "memory: frame allocator region clipped (>64GiB): {:#x?} -> {:#x?}",
                region,
                start..end
            );
        }
        let frame_start = phys_addr_to_frame_idx(start);
        let frame_end = phys_addr_to_frame_idx(end - 1) + 1;
        if frame_start < frame_end {
            ba.insert(frame_start..frame_end);
            TOTAL_MEMORY.fetch_add(
                frame_idx_to_phys_addr(frame_end - frame_start),
                Ordering::Relaxed,
            );
            let range_start = frame_idx_to_phys_addr(frame_start);
            let range_end = frame_idx_to_phys_addr(frame_end);
            let mib = (range_end - range_start) / (1024 * 1024);
            crate::klog_info!(
                "memory: free RAM range {:#x}..{:#x} ({} MiB)",
                range_start,
                range_end,
                mib
            );
        }
    }
    let (frames_used, frames_total) = frame_stats();
    crate::klog_info!(
        "memory: frame allocator ready ({} MiB managed, {} KiB used)",
        frames_total / (1024 * 1024),
        frames_used / 1024
    );
}

pub fn frame_alloc(frame_count: usize, align_log2: usize) -> Option<PhysAddr> {
    // Single-frame allocations (the page-fault/commit hot path — every
    // demand-paged page in the system) MUST use the cascade fast path.
    // `alloc_contiguous(1, 0)` goes through `find_contiguous`, which probes
    // linearly FROM INDEX 0 across the already-allocated region on every call:
    // O(allocated) per allocation, quadratic overall. Measured on hardware:
    // fork()'s per-page commit degraded 114 -> 240 us/page as allocation grew,
    // turning a 107 MiB mapping into 6.4 s of frame allocation (and the fork's
    // tail into an apparent freeze). `alloc()` descends the bitmap cascade via
    // trailing_zeros — O(log) — restoring ~us-scale commits.
    let start_idx = if frame_count == 1 && align_log2 == 0 {
        FRAME_ALLOCATOR.lock().alloc()
    } else {
        FRAME_ALLOCATOR
            .lock()
            .alloc_contiguous(frame_count, align_log2)
    };
    if let Some(idx) = start_idx {
        FRAMES_USED.fetch_add(frame_count << PAGE_BITS, Ordering::Relaxed);
        for i in 0..frame_count {
            track_mark_alloc(idx + i);
        }
    } else {
        // DIAGNOSTIC: a frame allocation failed. For a single-frame request
        // (the paged-VMO commit path) this means physical RAM is genuinely
        // exhausted; for a multi-frame request it may instead be fragmentation
        // (no contiguous run). Printed unconditionally so we can tell a real
        // shortage apart from a code path bug when something reports ENOMEM.
        let used = FRAMES_USED.load(Ordering::Relaxed);
        let total = TOTAL_MEMORY.load(Ordering::Relaxed);
        // error! (console), not klog: physical-RAM exhaustion during a fork's
        // eager copy surfaces as a mysterious ENOMEM/stall with nothing on
        // screen — this line is the difference between diagnosing it from a
        // photo and chasing ghosts.
        log::error!(
            "frame_alloc FAILED: count={} align_log2={} | {} MiB used / {} MiB managed",
            frame_count,
            align_log2,
            used / (1024 * 1024),
            total / (1024 * 1024),
        );
    }
    let ret = start_idx.map(frame_idx_to_phys_addr);
    trace!(
        "frame_alloc_contiguous(): {ret:x?} ~ {end_ret:x?}, align_log2={align_log2}",
        end_ret = ret.map(|x| x + frame_count),
    );
    ret
}

pub fn frame_dealloc(target: PhysAddr) {
    trace!("frame_dealloc(): {target:x}");
    let idx = phys_addr_to_frame_idx(target);
    // Marcar libre ANTES de devolverlo al allocator: si es un doble-free, lo
    // detectamos antes de reinsertarlo.
    track_mark_free(idx);
    // DEBUG: envenenar el frame con 0x5A al liberarlo. Si un proceso lee este
    // patrón en su memoria (fault a una dirección ~0x5a5a5a5a...), está leyendo
    // un frame YA LIBERADO -> PTE rancia (free-sin-unmap / use-after-free).
    //
    // Resolver la dirección del frame con `phys_to_virt` en lugar de la base
    // physmap hardcodeada: en bare-metal es el mismo offset, pero en libos la
    // "memoria física" es un mmap del proceso anfitrión en otra base — escribir
    // a `0xffff_8000_…` allí provoca un SIGSEGV del propio anfitrión.
    unsafe {
        core::ptr::write_bytes(
            kernel_hal::mem::phys_to_virt(target) as *mut u8,
            0x5A,
            1 << PAGE_BITS,
        );
    }
    FRAMES_USED.fetch_sub(1 << PAGE_BITS, Ordering::Relaxed);
    FRAME_ALLOCATOR.lock().dealloc(idx);
}

pub fn frame_stats() -> (usize, usize) {
    (
        FRAMES_USED.load(Ordering::Relaxed),
        TOTAL_MEMORY.load(Ordering::Relaxed),
    )
}

/// Reserve every frame of the CURRENTLY ACTIVE page-table tree so the frame
/// allocator can never hand them out as ordinary RAM.
///
/// Root cause of the desktop's escalating corruption: on x86_64 the kernel
/// keeps running on the page tables the BOOTLOADER built — and UEFI marks
/// that memory as reclaimable boot-services data, so `insert_regions` fed the
/// LIVE page-table frames into the allocator as free RAM. Nothing failed
/// until memory pressure (waybar's GTK heap) finally reused one of those
/// frames: translations then rotted progressively (kernel #GPs whose
/// registers held ELF file bytes, an all-idle wakeup wedge) and, once the
/// root PML4 itself was recycled, the next `activate_kernel_paging()` loaded
/// a CR3 whose tree was user data — instruction fetch of the kernel faulted,
/// the #DF handler was unfetchable for the same reason, and the machine
/// TRIPLE-FAULTED with no banner (QEMU `-d int` was needed to see it).
///
/// Must run right after `insert_regions`, before any frame allocation.
pub fn reserve_active_page_table_frames() {
    let root = kernel_hal::vm::current_vmtoken();
    if root == 0 {
        return; // libos / no paging context: nothing to reserve
    }
    let mut ba = FRAME_ALLOCATOR.lock();
    let mut reserved = 0usize;
    // Depth-first walk of the 4-level tree. Huge-page entries (PS bit) have no
    // lower table; entry bit 0 = present. Physical addresses are masked to 52
    // bits and page-aligned.
    fn walk(ba: &mut FrameAlloc, table_pa: usize, level: u8, reserved: &mut usize) {
        const PRESENT: u64 = 1;
        const HUGE: u64 = 1 << 7;
        let idx = table_pa >> PAGE_BITS;
        if table_pa != 0 && table_pa < MAX_MANAGED_PADDR_EXCLUSIVE {
            // `remove` marks the frame used regardless of current state.
            ba.remove(idx..idx + 1);
            *reserved += 1;
        }
        if level == 1 {
            return;
        }
        let entries = unsafe {
            core::slice::from_raw_parts(kernel_hal::mem::phys_to_virt(table_pa) as *const u64, 512)
        };
        for &e in entries {
            if e & PRESENT == 0 || (level < 4 && e & HUGE != 0) {
                continue;
            }
            let child = (e & 0x000f_ffff_ffff_f000) as usize;
            walk(ba, child, level - 1, reserved);
        }
    }
    walk(&mut ba, root, 4, &mut reserved);
    drop(ba);
    crate::klog_info!(
        "memory: reserved {} live page-table frame(s) of the boot tree (root {:#x})",
        reserved,
        root
    );
}

/// Combined usage for `/proc/meminfo` and diagnostics.
pub fn stats() -> (usize, usize) {
    let heap_used = HEAP_USED.load(Ordering::Relaxed);
    let frames_used = FRAMES_USED.load(Ordering::Relaxed);
    (
        heap_used + frames_used,
        heap_total() + TOTAL_MEMORY.load(Ordering::Relaxed),
    )
}

/// Kernel heap bytes currently allocated (diagnostics / OOM handler).
#[allow(dead_code)]
pub fn heap_used() -> usize {
    HEAP_USED.load(Ordering::Relaxed)
}

cfg_if! {
    if #[cfg(not(feature = "libos"))] {
        use buddy_system_allocator::Heap;
        use core::{
            alloc::{GlobalAlloc, Layout},
            ops::Deref,
            ptr::NonNull,
        };

        /// Kernel heap — separate from the physical frame pool.
        ///
        /// 512 MiB: the full desktop session (labwc + lunarbg + foot + waybar,
        /// each mapping dozens of glibc shared objects through the SFS block
        /// cache) OOMed the previous 256 MiB pool while the clients were still
        /// loading. The heap is a BSS static, so this costs nothing on disk and
        /// only reserves (zero-fill) RAM at boot.
        const KERNEL_HEAP_SIZE: usize = 512 * 1024 * 1024; // 512 MiB
        const ORDER: usize = 32;

        // Invariants: `init()` carves the heap into word-sized slots
        // (`HEAP_BLOCK = KERNEL_HEAP_SIZE / size_of::<usize>()`), so the size
        // must divide evenly by the machine word — otherwise the backing static
        // would silently under-provision the allocator. It must also be
        // page-aligned, since the region is handed to the allocator wholesale.
        const _: () = assert!(KERNEL_HEAP_SIZE % core::mem::size_of::<usize>() == 0);
        const _: () = assert!(KERNEL_HEAP_SIZE % 4096 == 0);

        #[global_allocator]
        static HEAP_ALLOCATOR: LockedHeap<ORDER> = LockedHeap::<ORDER>::new();

        pub fn heap_total() -> usize {
            KERNEL_HEAP_SIZE
        }

        pub fn init() {
            const MACHINE_ALIGN: usize = core::mem::size_of::<usize>();
            const HEAP_BLOCK: usize = KERNEL_HEAP_SIZE / MACHINE_ALIGN;
            static mut HEAP: [usize; HEAP_BLOCK] = [0; HEAP_BLOCK];
            let heap_start = core::ptr::addr_of_mut!(HEAP).cast::<u8>() as usize;
            unsafe {
                HEAP_ALLOCATOR
                    .lock()
                    .init(heap_start, HEAP_BLOCK * MACHINE_ALIGN);
            }
            crate::klog_info!(
                "memory: kernel heap ready ({} KiB @ {:#x})",
                KERNEL_HEAP_SIZE / 1024,
                heap_start
            );
        }

        pub struct LockedHeap<const ORDER: usize>(Mutex<Heap<ORDER>>);

        impl<const ORDER: usize> LockedHeap<ORDER> {
            pub const fn new() -> Self {
                LockedHeap(Mutex::new(Heap::<ORDER>::new()))
            }
        }

        impl<const ORDER: usize> Deref for LockedHeap<ORDER> {
            type Target = Mutex<Heap<ORDER>>;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        // Sin redzone: el canario de depuración (16 bytes tras cada asignación)
        // ya cumplió su función (la corrupción de heap que perseguía está
        // arreglada) y tenía un coste oculto brutal: el buddy allocator
        // redondea cada asignación a la potencia de dos superior, así que una
        // asignación de 4096 bytes (la clase dominante: caché de bloques del
        // SFS, buffers de pipe) pasaba a consumir 8192 reales. La sesión de
        // escritorio agotaba el pool con `HEAP_USED` marcando solo ~54%, porque
        // la contabilidad cuenta `sz` y no el bloque redondeado.

        // Histograma de asignaciones VIVAS por clase de tamaño (log2). El OOM
        // del escritorio (499/512 MiB usados) necesita atribución: qué clase
        // de tamaño retiene el heap. Coste: un fetch_add relajado por
        // alloc/dealloc. `heap_live_histogram` lo vuelca el alloc_error
        // handler sin asignar memoria.
        const HEAP_BUCKETS: usize = 32;
        static HEAP_LIVE: [AtomicUsize; HEAP_BUCKETS] = {
            const Z: AtomicUsize = AtomicUsize::new(0);
            [Z; HEAP_BUCKETS]
        };

        #[inline]
        fn bucket_of(size: usize) -> usize {
            (usize::BITS - size.max(1).leading_zeros()) as usize % HEAP_BUCKETS
        }

        /// Live-allocation counts per power-of-two size class, for the OOM
        /// report. Index i counts allocations with size in (2^(i-1), 2^i].
        pub fn heap_live_histogram() -> [usize; HEAP_BUCKETS] {
            let mut out = [0usize; HEAP_BUCKETS];
            for (i, slot) in HEAP_LIVE.iter().enumerate() {
                out[i] = slot.load(Ordering::Relaxed);
            }
            out
        }

        // Exact-size tracking for the OOM's dominant class: the first report
        // showed ~116k live allocations in the 4..8 KiB bucket eating the whole
        // heap, and the exact byte size is usually enough to name the struct
        // responsible. Track live counts for up to 16 distinct sizes in
        // [4096, 8192], first-come-first-served.
        const HOT_LO: usize = 4096;
        const HOT_HI: usize = 8192;
        const HOT_SLOTS: usize = 16;
        static HOT_SIZE: [AtomicUsize; HOT_SLOTS] = {
            const Z: AtomicUsize = AtomicUsize::new(0);
            [Z; HOT_SLOTS]
        };
        static HOT_LIVE: [AtomicUsize; HOT_SLOTS] = {
            const Z: AtomicUsize = AtomicUsize::new(0);
            [Z; HOT_SLOTS]
        };

        fn hot_track(size: usize, delta: isize) {
            if !(HOT_LO..=HOT_HI).contains(&size) {
                return;
            }
            for i in 0..HOT_SLOTS {
                let cur = HOT_SIZE[i].load(Ordering::Relaxed);
                let claimed = cur == size
                    || (cur == 0
                        && HOT_SIZE[i]
                            .compare_exchange(0, size, Ordering::Relaxed, Ordering::Relaxed)
                            .map_or_else(|racer| racer == size, |_| true));
                if claimed {
                    if delta > 0 {
                        let live = HOT_LIVE[i].fetch_add(1, Ordering::Relaxed) + 1;
                        // Leak-site sampler: the desktop OOM is ~117k live
                        // 4096-byte allocations. There is no panic backtrace in
                        // this kernel, so when the live count crosses a
                        // threshold, scan the current stack for kernel-text
                        // return addresses and print them — a poor man's
                        // backtrace naming the allocating call chain
                        // (resolve offline with addr2line).
                        if size == 4096 && (live == 50_000 || live == 90_000) {
                            leak_trace_dump(live);
                        }
                    } else {
                        HOT_LIVE[i].fetch_sub(1, Ordering::Relaxed);
                    }
                    return;
                }
            }
        }

        /// Print kernel-.text-looking words found on the current stack (the
        /// return-address chain of whoever is allocating), via the no-alloc
        /// spin serial writer. Reads stay inside the mapped kernel heap /
        /// physmap, so over-scanning past the coroutine stack top is safe.
        #[cold]
        fn leak_trace_dump(live: usize) {
            let mut rsp: usize;
            unsafe { core::arch::asm!("mov {}, rsp", out(reg) rsp) };
            kernel_hal::console::serial_write_fmt_spin(format_args!(
                "\n[leaktrace] 4096B live={} stack-scan:",
                live
            ));
            const TEXT_LO: usize = 0xffffff00_0000_1000;
            const TEXT_HI: usize = 0xffffff00_0100_0000;
            let mut printed = 0;
            let mut p = rsp;
            while printed < 24 && p < rsp + 32 * 1024 {
                let v = unsafe { core::ptr::read_volatile(p as *const usize) };
                if (TEXT_LO..TEXT_HI).contains(&v) {
                    kernel_hal::console::serial_write_fmt_spin(format_args!(" {:#x}", v));
                    printed += 1;
                }
                p += 8;
            }
            kernel_hal::console::serial_write_fmt_spin(format_args!("\n"));
        }

        /// (size, live-count) pairs for the exact-size tracker (0 = unused slot).
        pub fn heap_hot_sizes() -> [(usize, usize); HOT_SLOTS] {
            let mut out = [(0usize, 0usize); HOT_SLOTS];
            for i in 0..HOT_SLOTS {
                out[i] = (
                    HOT_SIZE[i].load(Ordering::Relaxed),
                    HOT_LIVE[i].load(Ordering::Relaxed),
                );
            }
            out
        }

        // Heap-corruption forensics, reinstated after the desktop soak kept
        // dying on #GPs whose registers held ELF-magic bytes (a freed heap
        // block reused as a file buffer while its old owner still points at
        // it). Two tools:
        //  * REDZONE canary after every allocation: linear overflows panic at
        //    dealloc naming the clobbered block. (This was removed earlier
        //    because the buddy's power-of-two rounding doubled the dominant
        //    4 KiB class; with the ramfs now sparse the headroom is back.)
        //  * Poison-on-free (0xA5): a use-after-free READ yields the
        //    unmistakable 0xa5a5... pattern in crash registers instead of
        //    whatever the next owner wrote, separating "read after free" from
        //    "read after free AND reuse".
        const REDZONE: usize = 16;
        const CANARY: u8 = 0xAB;
        const POISON: u8 = 0xA5;

        unsafe impl<const ORDER: usize> GlobalAlloc for LockedHeap<ORDER> {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                let sz = layout.size();
                let ext = Layout::from_size_align_unchecked(sz + REDZONE, layout.align());
                self.0
                    .lock()
                    .alloc(ext)
                    .ok()
                    .map_or(core::ptr::null_mut::<u8>(), |allocation| {
                        HEAP_USED.fetch_add(sz, Ordering::Relaxed);
                        HEAP_LIVE[bucket_of(sz)].fetch_add(1, Ordering::Relaxed);
                        hot_track(sz, 1);
                        let p = allocation.as_ptr();
                        let cz = p.add(sz);
                        for i in 0..REDZONE {
                            core::ptr::write_volatile(cz.add(i), CANARY);
                        }
                        p
                    })
            }

            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                let sz = layout.size();
                let cz = ptr.add(sz);
                for i in 0..REDZONE {
                    if core::ptr::read_volatile(cz.add(i)) != CANARY {
                        panic!(
                            "[heapcanary] HEAP OVERFLOW: ptr={:#x} size={} align={} clobbered at +{} (val={:#x})",
                            ptr as usize,
                            sz,
                            layout.align(),
                            i,
                            core::ptr::read_volatile(cz.add(i))
                        );
                    }
                }
                // Poison the payload before returning it to the buddy so a
                // stale reader sees 0xa5a5... instead of plausible data.
                core::ptr::write_bytes(ptr, POISON, sz);
                HEAP_USED.fetch_sub(sz, Ordering::Relaxed);
                HEAP_LIVE[bucket_of(sz)].fetch_sub(1, Ordering::Relaxed);
                hot_track(sz, -1);
                let ext = Layout::from_size_align_unchecked(sz + REDZONE, layout.align());
                self.0.lock().dealloc(NonNull::new_unchecked(ptr), ext)
            }
        }
    } else {
        pub fn init() {}

        pub fn heap_total() -> usize {
            0
        }
    }
}

#[cfg(feature = "hypervisor")]
mod rvm_extern_fn {
    use super::*;

    #[rvm::extern_fn(alloc_frame)]
    fn rvm_alloc_frame() -> Option<usize> {
        hal_frame_alloc()
    }

    #[rvm::extern_fn(dealloc_frame)]
    fn rvm_dealloc_frame(paddr: usize) {
        hal_frame_dealloc(&paddr)
    }

    #[rvm::extern_fn(phys_to_virt)]
    fn rvm_phys_to_virt(paddr: usize) -> usize {
        paddr + PHYSICAL_MEMORY_OFFSET
    }

    #[cfg(target_arch = "x86_64")]
    #[rvm::extern_fn(is_host_timer_interrupt)]
    fn rvm_is_host_timer_interrupt(vector: u8) -> bool {
        vector == 32
    }

    #[cfg(target_arch = "x86_64")]
    #[rvm::extern_fn(is_host_serial_interrupt)]
    fn rvm_is_host_serial_interrupt(vector: u8) -> bool {
        vector == 36
    }
}

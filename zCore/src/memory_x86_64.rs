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
#[cfg(feature = "mem-debug")]
use core::sync::atomic::AtomicU64;
use core::sync::atomic::{AtomicUsize, Ordering};
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
//
// Gated (feature `mem-debug`): un RMW SeqCst por frame en alloc Y free, en la
// ruta de commit de página — el camino más caliente del sistema de memoria.
#[cfg(feature = "mem-debug")]
const TRACK_MAX_FRAMES: usize = 1 << 20; // 4 GiB / 4 KiB
#[cfg(feature = "mem-debug")]
const TRACK_WORDS: usize = TRACK_MAX_FRAMES / 64;
#[cfg(feature = "mem-debug")]
static FRAME_ALLOCATED: [AtomicU64; TRACK_WORDS] = [const { AtomicU64::new(0) }; TRACK_WORDS];

#[cfg(feature = "mem-debug")]
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

#[cfg(feature = "mem-debug")]
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
        #[cfg(feature = "mem-debug")]
        for i in 0..frame_count {
            track_mark_alloc(idx + i);
        }
        #[cfg(not(feature = "mem-debug"))]
        let _ = idx;
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
    #[cfg(feature = "mem-debug")]
    track_mark_free(idx);
    // DEBUG (feature `mem-debug`): envenenar el frame con 0x5A al liberarlo. Si
    // un proceso lee este patrón en su memoria (fault a una dirección
    // ~0x5a5a5a5a...), está leyendo un frame YA LIBERADO -> PTE rancia
    // (free-sin-unmap / use-after-free).
    //
    // Resolver la dirección del frame con `phys_to_virt` en lugar de la base
    // physmap hardcodeada: en bare-metal es el mismo offset, pero en libos la
    // "memoria física" es un mmap del proceso anfitrión en otra base — escribir
    // a `0xffff_8000_…` allí provoca un SIGSEGV del propio anfitrión.
    //
    // Coste sin la feature: nada — el memset de 4 KiB por frame liberado
    // duplicaba el tráfico de memoria de todo ciclo alloc/free (el commit de
    // página ya rellena a cero al asignar).
    #[cfg(feature = "mem-debug")]
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

        /// One-shot report that the buddy allocator just dispensed a block
        /// overlapping a live coroutine stack — the double-alloc every
        /// null-range crash has been chasing. Runs inside `alloc`, on the
        /// allocating call chain, so its backtrace names the writer's path.
        #[cold]
        #[inline(never)]
        fn report_stack_double_alloc(ptr: usize, sz: usize, stack_base: usize) {
            use core::sync::atomic::{AtomicBool, Ordering};
            executor::note_heap_smash_suspected();
            static REPORTED: AtomicBool = AtomicBool::new(false);
            if REPORTED.swap(true, Ordering::SeqCst) {
                return;
            }
            kernel_hal::console::serial_write_fmt_spin(format_args!(
                "\n[double-alloc] BUDDY HANDED OUT A LIVE COROUTINE STACK: \
                 alloc ptr={:#x} size={:#x} overlaps live stack alloc_base={:#x}. \
                 This allocation is the wild zero-writer; its call chain is below \
                 (diag rev 4).\n",
                ptr, sz, stack_base,
            ));
            // Frame-pointer backtrace of the ALLOCATING path — the code about
            // to zero-fill this block over a live stack. Bounded and guarded so
            // a corrupt chain cannot fault this report.
            let mut rbp: usize;
            unsafe { core::arch::asm!("mov {}, rbp", out(reg) rbp) };
            for _ in 0..20 {
                if rbp == 0 || rbp & 0x7 != 0 || rbp < 0xffff_ff00_0000_0000 {
                    break;
                }
                let ret = unsafe { core::ptr::read_volatile((rbp + 8) as *const usize) };
                let next = unsafe { core::ptr::read_volatile(rbp as *const usize) };
                if ret == 0 {
                    break;
                }
                kernel_hal::console::serial_write_fmt_spin(format_args!(
                    "[double-alloc]   ret={:#x}\n",
                    ret
                ));
                if next <= rbp {
                    break;
                }
                rbp = next;
            }
            kernel_hal::console::serial_write_str(
                "[double-alloc] symbolize: llvm-addr2line -e <zcore.elf> -fCi <ret ...>\n",
            );
        }

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
            // Teach the fat-pointer liveness gate where the heap begins. A real
            // vtable lives in `.rodata`, always below this address; any dyn
            // pointer whose vtable word lands at or above it is a use-after-free
            // and must be leaked, not dispatched. See
            // `zcore_drivers::utils::fat_ptr`.
            kernel_hal::drivers::utils::set_vtable_max(heap_start);
            crate::klog_info!(
                "memory: kernel heap ready ({} KiB @ {:#x}), vtable ceiling registered",
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

        // Exact-size tracking for OOM attribution. Track live counts for up to 32
        // distinct allocation sizes across the heap, first-come-first-served.
        const HOT_SLOTS: usize = 32;
        static HOT_SIZE: [AtomicUsize; HOT_SLOTS] = {
            const Z: AtomicUsize = AtomicUsize::new(0);
            [Z; HOT_SLOTS]
        };
        static HOT_LIVE: [AtomicUsize; HOT_SLOTS] = {
            const Z: AtomicUsize = AtomicUsize::new(0);
            [Z; HOT_SLOTS]
        };

        fn hot_track(size: usize, delta: isize) {
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
                        if size == 4096 && (live == 50_000 || live == 90_000) {
                            leak_trace_dump(live);
                        }
                    } else {
                        HOT_LIVE[i].fetch_update(
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                            |v| Some(v.saturating_sub(1)),
                        ).ok();
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

        // Heap-corruption forensics (feature `mem-debug`), reinstated after the
        // desktop soak kept dying on #GPs whose registers held ELF-magic bytes
        // (a freed heap block reused as a file buffer while its old owner
        // still points at it). Two tools:
        //  * REDZONE canary after every allocation: linear overflows panic at
        //    dealloc naming the clobbered block. (Hidden cost: the buddy's
        //    power-of-two rounding means the +16 doubles the real footprint of
        //    the dominant 4 KiB class — SFS block cache, pipe buffers.)
        //  * Poison-on-free (0xA5): a use-after-free READ yields the
        //    unmistakable 0xa5a5... pattern in crash registers instead of
        //    whatever the next owner wrote, separating "read after free" from
        //    "read after free AND reuse". (Hidden cost: dealloc becomes
        //    O(size) — freeing a 64 KiB I/O buffer memsets all 64 KiB.)
        //
        // Default build: no redzone, no poison — alloc/dealloc are the buddy
        // op plus two relaxed counters.
        #[cfg(feature = "mem-debug")]
        const REDZONE: usize = 16;
        #[cfg(not(feature = "mem-debug"))]
        const REDZONE: usize = 0;
        #[cfg(feature = "mem-debug")]
        const CANARY: u8 = 0xAB;
        #[cfg(feature = "mem-debug")]
        const POISON: u8 = 0xA5;

        // ── Front cache: per-size-class free lists over the buddy allocator ──
        //
        // The buddy's `dealloc` coalesces by SCANNING `free_list[class]` for the
        // sibling block (buddy_system_allocator 0.8 lib.rs:161) — O(free-list
        // length). A fork storm allocates and frees thousands of same-sized
        // objects (VMObjectPaged inner, VmMapping, VmMappingInner) whose buddies
        // are still live, so those class free lists grow long and *every*
        // dealloc pays O(n): the fork loop is O(n²). HEAPPROF confirmed it —
        // dealloc climbed from ~18 K to ~284 K cyc/call across a 512-map loop.
        //
        // This cache keeps recently-freed small blocks in per-class LIFO stacks
        // (the next pointer lives in the freed block's first word, like the
        // buddy's own list) and serves allocations from them in O(1). The common
        // alloc/free pair never touches the buddy, so the buddy free lists stay
        // short and the O(n) scan never fires on the hot path. The buddy is
        // consulted only to refill an empty class or to absorb frees past the
        // per-class cap.
        //
        // Safety hinges on one buddy invariant: every block of class `c` is
        // 2^c-aligned. The buddy carves its region by the address' low bit and
        // splits power-of-two blocks in halves (lib.rs:87, 117), so a class-`c`
        // block is always 2^c-aligned. Since `class_size ≥ layout.align()`, any
        // cached class-`c` block satisfies the alignment of *any* allocation that
        // maps to class `c` — blocks in a class are freely interchangeable.
        //
        // Only enabled in the default build. `mem-debug` wants real buddy
        // round-trips for its canary/poison forensics, so it bypasses the cache.
        #[cfg(not(feature = "mem-debug"))]
        mod slab {
            use super::Mutex;
            use core::alloc::Layout;

            // Cache buddy classes 2^3 (8 B, the buddy minimum) .. 2^12 (4 KiB).
            // That covers every hot fork object and the general small-allocation
            // churn; larger buffers (I/O, 1 MiB readahead) churn rarely and would
            // pin too much idle RAM, so they go straight to the buddy.
            const MIN_CLASS: usize = 3; // 2^3 = 8 B
            const MAX_CLASS: usize = 12; // 2^12 = 4 KiB
            const NUM_CLASSES: usize = MAX_CLASS - MIN_CLASS + 1;
            // Per-class cap. Worst case is the top class: 4 KiB * 1024 = 4 MiB;
            // the full table pins ≈8 MiB out of a 512 MiB heap. Bounding it keeps
            // the buddy from starving of large contiguous blocks and stops idle
            // RAM accumulating in the cache. A freed block past the cap is handed
            // to the buddy (where its buddy may coalesce) instead of cached.
            const CAP: u32 = 1024;

            pub struct SlabCache {
                // head[i]: first free block of class MIN_CLASS+i (0 = empty). The
                // block's first word holds the next pointer while it is cached.
                head: [usize; NUM_CLASSES],
                count: [u32; NUM_CLASSES],
            }

            impl SlabCache {
                const fn new() -> Self {
                    SlabCache {
                        head: [0; NUM_CLASSES],
                        count: [0; NUM_CLASSES],
                    }
                }
            }

            static SLAB: Mutex<SlabCache> = Mutex::new(SlabCache::new());

            /// Cache-array index for `layout`, or `None` when it is larger than
            /// the top cached class. The class formula matches
            /// buddy_system_allocator exactly, so a cached block is byte-for-byte
            /// what the buddy would have handed out.
            #[inline]
            fn cache_slot(layout: Layout) -> Option<usize> {
                // Guard before `next_power_of_two` so an oversize request can
                // never overflow it (and matches the buddy's own bypass of the
                // cache for large blocks).
                if layout.size() > (1 << MAX_CLASS) || layout.align() > (1 << MAX_CLASS) {
                    return None;
                }
                let size = core::cmp::max(
                    layout.size().next_power_of_two(),
                    core::cmp::max(layout.align(), core::mem::size_of::<usize>()),
                );
                // size ∈ [8, 4096] given the guard, so class ∈ [3, 12].
                Some(size.trailing_zeros() as usize - MIN_CLASS)
            }

            /// Serve `layout` from the front cache, or null on a miss. A pure
            /// block-provider — the caller owns the live-heap accounting.
            ///
            /// Uses `try_lock` instead of `lock` so that re-entrant callers
            /// (e.g. an interrupt that fires while the same CPU already holds
            /// SLAB) get a cache miss and fall through to the buddy allocator
            /// rather than spinning forever on a lock they already own.
            #[inline]
            pub fn try_alloc(layout: Layout) -> *mut u8 {
                let Some(i) = cache_slot(layout) else {
                    return core::ptr::null_mut();
                };
                let Some(mut c) = SLAB.try_lock() else {
                    return core::ptr::null_mut();
                };
                let head = c.head[i];
                if head == 0 {
                    return core::ptr::null_mut();
                }
                // Pop: the block's first word is the next pointer.
                c.head[i] = unsafe { core::ptr::read(head as *const usize) };
                c.count[i] -= 1;
                head as *mut u8
            }

            /// Return a freed block to the front cache. `false` means the class is
            /// out of range, the cache is full, or the lock is already held on
            /// this CPU — in all cases the caller must hand the block to the buddy.
            ///
            /// Uses `try_lock` instead of `lock` for the same re-entrancy safety
            /// reason as `try_alloc`.
            #[inline]
            pub fn try_free(ptr: *mut u8, layout: Layout) -> bool {
                let Some(i) = cache_slot(layout) else {
                    return false;
                };
                let Some(mut c) = SLAB.try_lock() else {
                    return false;
                };
                if c.count[i] >= CAP {
                    return false;
                }
                // Push: stash the old head in the block's first word.
                unsafe { core::ptr::write(ptr as *mut usize, c.head[i]) };
                c.head[i] = ptr as usize;
                c.count[i] += 1;
                true
            }
        }

        unsafe impl<const ORDER: usize> GlobalAlloc for LockedHeap<ORDER> {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                let sz = layout.size();
                let ext = Layout::from_size_align_unchecked(sz + REDZONE, layout.align());
                // See `kernel_hal::kstats::heap_prof_enabled`: off by default,
                // and when off this is one relaxed load on the kernel's hottest
                // path.
                let prof = kernel_hal::kstats::heap_prof_enabled();
                let t0 = if prof { core::arch::x86_64::_rdtsc() } else { 0 };
                // Front cache first (default build); a miss falls through to the
                // buddy. The cache serves the common small alloc/free pair in
                // O(1), keeping the buddy's class free lists short so its O(n)
                // coalescing scan never fires on the fork hot path.
                #[cfg(not(feature = "mem-debug"))]
                let p = {
                    let cached = slab::try_alloc(ext);
                    if cached.is_null() {
                        self.0
                            .lock()
                            .alloc(ext)
                            .ok()
                            .map_or(core::ptr::null_mut::<u8>(), |a| a.as_ptr())
                    } else {
                        cached
                    }
                };
                #[cfg(feature = "mem-debug")]
                let p = self
                    .0
                    .lock()
                    .alloc(ext)
                    .ok()
                    .map_or(core::ptr::null_mut::<u8>(), |a| a.as_ptr());
                if !p.is_null() {
                    // [diag] Double-alloc tripwire. Every crash zeroes a live
                    // coroutine stack; if the buddy hands out a block that
                    // overlaps one, this is the writer's own allocation and its
                    // call chain is on the stack right now. Report once with a
                    // frame-pointer backtrace and latch the smash flag so the
                    // timer path stops running on the doomed stack. Cheap: a
                    // scan of the small live-executor set per allocation.
                    if let Some(base) = executor::alloc_overlaps_live_stack(p as usize, sz + REDZONE)
                    {
                        report_stack_double_alloc(p as usize, sz, base);
                    }
                    HEAP_USED.fetch_add(sz, Ordering::Relaxed);
                    HEAP_LIVE[bucket_of(sz)].fetch_add(1, Ordering::Relaxed);
                    hot_track(sz, 1);
                    #[cfg(feature = "mem-debug")]
                    {
                        let cz = p.add(sz);
                        for i in 0..REDZONE {
                            core::ptr::write_volatile(cz.add(i), CANARY);
                        }
                    }
                }
                kernel_hal::kstats::note_heap_alloc(if prof {
                    core::arch::x86_64::_rdtsc().wrapping_sub(t0)
                } else {
                    0
                });
                p
            }

            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                let prof = kernel_hal::kstats::heap_prof_enabled();
                let t0 = if prof { core::arch::x86_64::_rdtsc() } else { 0 };
                let sz = layout.size();
                hot_track(sz, -1);
                #[cfg(feature = "mem-debug")]
                {
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
                }
                HEAP_USED.fetch_sub(sz, Ordering::Relaxed);
                HEAP_LIVE[bucket_of(sz)].fetch_sub(1, Ordering::Relaxed);
                let ext = Layout::from_size_align_unchecked(sz + REDZONE, layout.align());
                // Front cache absorbs the free in O(1); only an out-of-range size
                // or a cap-overflow reaches the buddy (default build only).
                #[cfg(not(feature = "mem-debug"))]
                let to_buddy = !slab::try_free(ptr, ext);
                #[cfg(feature = "mem-debug")]
                let to_buddy = true;
                if to_buddy {
                    self.0.lock().dealloc(NonNull::new_unchecked(ptr), ext);
                }
                kernel_hal::kstats::note_heap_dealloc(if prof {
                    core::arch::x86_64::_rdtsc().wrapping_sub(t0)
                } else {
                    0
                });
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

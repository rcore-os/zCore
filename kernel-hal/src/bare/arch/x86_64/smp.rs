//! x86_64 Symmetric Multi-Processing startup.
//!
//! Brings up application processors using the LAPIC INIT/SIPI sequence.
//! The AP trampoline is assembled inline (avoiding x86-smpboot's Rust code
//! which causes R_X86_64_64 against local symbol link errors).
//! AP LAPIC IDs are enumerated from the ACPI MADT.

use alloc::vec::Vec;
use core::arch::global_asm;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use acpi::{AcpiHandler, AcpiTables, PhysicalMapping};
use x86::controlregs::cr3;

use crate::{mem::phys_to_virt, CachePolicy, MMUFlags, KCONFIG};

const PAGE_SIZE: usize = 4096;

// Per-AP stacks: 256 KB each, allocated from the kernel heap on demand.
const STACK_SIZE: usize = 256 * 1024;
// AP stacks are allocated via `Layout::from_size_align(STACK_SIZE, PAGE_SIZE)`,
// which requires the size to be a multiple of the (page) alignment.
const _: () = assert!(STACK_SIZE % PAGE_SIZE == 0);
// At most MAX_CORE_NUM logical CPUs total; the BSP is logical 0, so up to
// MAX_CORE_NUM - 1 application processors.
const MAX_APS: usize = crate::config::MAX_CORE_NUM - 1;

/// Best-effort cap (in ~1 ms rdtsc-spin iterations) for the single barrier that
/// waits for every launched AP to come online. Sized so genuinely working APs on
/// real hardware — which come up in parallel within tens of milliseconds — are
/// always caught, while a straggler starved under single-threaded QEMU TCG costs
/// at most this much boot time before we proceed and let it join during the
/// subsequent I/O. The system tolerates a late join (see the barrier comment),
/// so this is a nicety for a populated online set, not a correctness gate.
const AP_ONLINE_WAIT_MS: usize = 2_000;

// Trampoline lives at physical 0x6000; SIPI vector = 6
const TRAMPOLINE_PADDR: usize = 0x6000;
const SIPI_VECTOR: u8 = 6;

// Data slots in the trampoline page (physical addresses):
const SLOT_LOGICAL: usize = 0x6FD8; // u8: dense logical CPU id for the starting AP
const SLOT_STACK: usize = 0x6FE8; // usize: AP initial RSP
const SLOT_ENTRY: usize = 0x6FF0; // usize: 64-bit entry function
const SLOT_CR3: usize = 0x6FF8; // u32:   BSP CR3 (PML4 physical)

// ─── Trampoline assembly ──────────────────────────────────────────────────────
//
// Copied verbatim from x86-smpboot/src/boot_ap.S and included here so only
// the assembly object is linked (not the crate's Rust helper functions, which
// emit R_X86_64_64 against local symbols that the bare-metal linker rejects).
//
// Memory map within physical page 0x6000:
//   +0x000 : 16-bit/32-bit/64-bit trampoline code  (from ap_trampoline_start)
//   +0x6FE8: SLOT_STACK (stack top)
//   +0x6FF0: SLOT_ENTRY (entry fn ptr)
//   +0x6FF8: SLOT_CR3   (PML4 physical addr, u32)

global_asm!(
    // ── Symbolic constants (identical to boot_ap.S) ──
    ".equ ap_start64_paddr,      ap_trampoline64 - ap_trampoline_start + 0x6000",
    ".equ gdt_64_paddr,          gdt_64_smp     - ap_trampoline_start + 0x6000",
    ".equ gdt_64_pointer_paddr,  gdt_64_ptr_smp - ap_trampoline_start + 0x6000",
    ".equ cr3_ptr,   0x6ff8",
    ".equ entry_ptr, 0x6ff0",
    ".equ stack_ptr, 0x6fe8",
    ".equ temp_stack_top, 0x6fe0",
    ".section .text",
    ".code16",
    ".global ap_trampoline_start",
    ".global ap_trampoline_end",
    "ap_trampoline_start:",
    "  cli",
    "  xor  ax, ax",
    "  mov  ds, ax",
    "  mov  es, ax",
    "  mov  ss, ax",
    // CR4: PAE(5) | PGE(7) | OSFXSR(9) | OSXMMEXCPT(10)
    "  mov  eax, cr4",
    "  or   eax, (1 << 5) | (1 << 7) | (1 << 9) | (1 << 10)",
    "  mov  cr4, eax",
    // CR3
    "  mov  eax, [cr3_ptr]",
    "  mov  cr3, eax",
    // EFER: LME(8) | NXE(11)
    "  mov  ecx, 0xC0000080",
    "  rdmsr",
    "  or   eax, (1 << 8) | (1 << 11)",
    "  wrmsr",
    // CR0: PE(0) | MP(1) | WP(16) | PG(31)
    //
    // WP (Write Protect, bit 16) is LOAD-BEARING and must match the BSP, which
    // runs with WP=1 (rboot re-enables it before entering the kernel). With
    // WP=0 a *supervisor*-mode store to a read-only page SUCCEEDS SILENTLY
    // instead of faulting — so a kernel `copy_to_user` (e.g. recvfrom writing
    // downloaded bytes) onto a page that maps the shared read-only ZERO_FRAME
    // (a demand-zero heap page that was read-faulted first) would write into
    // the GLOBAL ZERO_FRAME instead of triggering the copy-on-write that a
    // WP=1 fault performs. That poisons every future demand-zero page, and
    // allocators that assume fresh pages are zero (apk's mimalloc: "corrupted
    // free list entry") abort deterministically. It only surfaced once anon
    // mmap became demand-paged (before that every anon page owned a private
    // frame, so the AP's WP=0 write was harmless); it is also a general
    // integrity hole against any AP-side write to shared RO pages (library
    // .text/.rodata), so the APs must carry WP=1 exactly like the BSP.
    "  mov  eax, cr0",
    "  or   eax, (1 << 0) | (1 << 1) | (1 << 16) | (1 << 31)",
    "  mov  cr0, eax",
    // Temporary stack
    "  mov  esp, temp_stack_top",
    // Load 64-bit GDT
    "  lgdt [gdt_64_pointer_paddr]",
    // Far-jump to 64-bit code. Encoded by hand (66 EA imm32 sel16): the
    // 16-bit push/retf pair mixes operand sizes and is fragile across
    // hypervisors/emulators; the direct ljmpl is the canonical switch.
    "  .byte 0x66, 0xea",
    "  .long ap_start64_paddr",
    "  .word 0x8",
    ".code64",
    "ap_trampoline64:",
    "  xor  ax, ax",
    "  mov  ss, ax",
    "  mov  ds, ax",
    "  mov  es, ax",
    "  mov  fs, ax",
    "  mov  gs, ax",
    "  mov  rsp, [stack_ptr]",
    "  mov  rax, [entry_ptr]",
    "  call rax",
    "1:",
    "  hlt",
    "  jmp 1b",
    // GDT
    ".align 4",
    "gdt_64_smp:",
    "  .quad 0x0000000000000000", // null
    "  .quad 0x00209A0000000000", // 64-bit code
    "  .quad 0x0000920000000000", // 64-bit data
    ".align 4",
    "  .word 0", // padding
    "gdt_64_ptr_smp:",
    "  .word gdt_64_ptr_smp - gdt_64_smp - 1",
    "  .long gdt_64_paddr",
    "ap_trampoline_end:",
    ".code64", // restore default for remaining file
);

extern "C" {
    fn ap_trampoline_start();
    fn ap_trampoline_end();
}

// ─── AP stacks ───────────────────────────────────────────────────────────────

/// Allocate a zeroed, page-aligned AP stack from the kernel heap and return its
/// top address. Leaked intentionally: AP stacks live for the lifetime of the CPU.
fn alloc_ap_stack() -> Option<usize> {
    use alloc::alloc::{alloc_zeroed, Layout};
    let layout = Layout::from_size_align(STACK_SIZE, PAGE_SIZE).unwrap();
    let base = unsafe { alloc_zeroed(layout) };
    if base.is_null() {
        return None;
    }
    Some(base as usize + STACK_SIZE)
}

/// Number of APs that have signalled they are running.
pub static AP_ONLINE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Handshake flag: set by the AP currently starting once it has *latched* its
/// logical id (and stack/entry) out of the shared trampoline slots. The
/// trampoline data slots (`SLOT_LOGICAL`/`SLOT_STACK`/`SLOT_ENTRY`) live at
/// fixed physical addresses and are reused for every AP, so the BSP must not
/// overwrite them for the next AP until the current one has copied them into
/// its own registers/locals. Without this, under single-threaded TCG (where a
/// busy-waiting BSP can outrun a slow AP — the per-CPU PIT freq calibration
/// alone spins ~55 ms) the BSP would clobber `SLOT_LOGICAL` mid-flight and two
/// APs would read the *same* logical id, sharing one PercpuBlock/GS/scheduler
/// slot — silent cross-CPU memory corruption.
static AP_SLOT_CONSUMED: AtomicBool = AtomicBool::new(false);

/// Called by the starting AP (from `secondary_init`) the instant it has copied
/// its logical id out of the trampoline slot. Releases the BSP to reuse the
/// shared trampoline slots for the next AP.
pub fn ap_signal_slot_consumed() {
    AP_SLOT_CONSUMED.store(true, Ordering::Release);
}

// ─── CPU topology: dense logical id  <->  Local APIC ID ─────────────────────────
//
// Local APIC IDs are sparse (cores/threads/sockets leave gaps), so they cannot be
// used directly to index per-CPU arrays. We assign each online CPU a dense logical
// id (0..NCPU, BSP = 0). The forward map (apic -> logical) lives in `lock` so the
// lock crate and the kernel share one id space; here we keep the reverse map
// (logical -> apic) needed to direct IPIs to the right hardware APIC.

/// logical id -> Local APIC ID. Index 0 is the BSP.
///
/// 32-bit, not 8-bit: in x2APIC mode APIC IDs are full 32-bit values and a
/// machine with more than 255 of them (or firmware that simply numbers them
/// sparsely above 255) would alias two CPUs onto one entry, sending every IPI
/// for one of them — TLB shootdowns included — to the wrong core.
static LOGICAL_TO_APIC: [AtomicU32; crate::config::MAX_CORE_NUM] =
    [const { AtomicU32::new(0) }; crate::config::MAX_CORE_NUM];

/// Bitmask of logical ids that [`register_cpu`] has wired. Needed because
/// LAPIC id 0 is a valid BSP id — cannot use 0 as "unset" in LOGICAL_TO_APIC.
static LOGICAL_REGISTERED: AtomicU64 = AtomicU64::new(0);

/// Number of logical ids assigned so far (next id to hand out).
pub(super) static CPU_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Raw Local APIC ID of the calling CPU (x2APIC MSR, else MMIO, else CPUID).
fn raw_apic_id() -> u32 {
    lock::hardware_apic_id()
}

/// Assign the next dense logical id to `apic_id`, wiring up both the forward map
/// (apic -> logical, owned by `lock`) and the reverse map (logical -> apic).
/// Returns the assigned logical id. Must run before the target CPU executes any
/// lock-taking code.
fn register_cpu(apic_id: u32) -> usize {
    let logical = CPU_COUNT.fetch_add(1, Ordering::AcqRel);
    assert!(
        logical < crate::config::MAX_CORE_NUM,
        "[smp] more online CPUs than MAX_CORE_NUM={}",
        crate::config::MAX_CORE_NUM
    );
    LOGICAL_TO_APIC[logical].store(apic_id, Ordering::Release);
    LOGICAL_REGISTERED.fetch_or(1u64 << logical, Ordering::Release);
    lock::set_logical_cpu_id(apic_id, logical as u8);
    logical
}

/// Undo the most recent [`register_cpu`] for `logical`.
///
/// Used when an AP that had already been assigned an id turns out not to be
/// startable (no stack, never latched its trampoline slot). Leaving the id
/// registered inflates `cpu_count()` — which userspace reads through
/// `/proc/cpuinfo` and `sched_getaffinity` — with a core that will never run,
/// and leaves a `logical_to_apic` entry that invites IPIs to a dead CPU.
fn unregister_cpu(logical: usize) {
    LOGICAL_REGISTERED.fetch_and(!(1u64 << logical), Ordering::Release);
    // Only the BSP hands out ids, and only the id it just took can be given
    // back; anything else would punch a hole in the dense numbering.
    let _ = CPU_COUNT.compare_exchange(logical + 1, logical, Ordering::AcqRel, Ordering::Relaxed);
}

/// Called by a starting AP once its own LAPIC is fully configured: publish the
/// APIC id the hardware actually reports for this CPU.
///
/// Two earlier sources are only provisional. The BSP seeds `logical -> apic`
/// from the ACPI MADT before the AP runs, and the AP's first self-registration
/// happens while its LAPIC is still in xAPIC mode (INIT leaves every AP there
/// no matter which mode the BSP is in), so it can only see an 8-bit id. If
/// either disagrees with the truth, IPIs to this CPU — TLB shootdowns included
/// — are addressed to a core that does not exist, and the shootdown initiator
/// waits for an acknowledgement that can never come.
pub fn ap_confirm_apic_id(logical: u8) {
    let idx = logical as usize;
    if idx >= crate::config::MAX_CORE_NUM {
        return;
    }
    let actual = raw_apic_id();
    let expected = LOGICAL_TO_APIC[idx].load(Ordering::Acquire);
    if expected != actual {
        crate::klog_warn!(
            "[smp] logical CPU {}: expected LAPIC {:#x}, hardware reports {:#x} — \
             using the hardware id for IPI delivery",
            idx,
            expected,
            actual
        );
        LOGICAL_TO_APIC[idx].store(actual, Ordering::Release);
    }
    LOGICAL_REGISTERED.fetch_or(1u64 << idx, Ordering::Release);
    // Keep the `lock` crate's apic -> logical map in step, so the pre-GS
    // fallback resolves this CPU by the same id the rest of the kernel uses.
    lock::set_logical_cpu_id(actual, logical);
}

/// Translate a dense logical CPU id back to its Local APIC ID (for IPI delivery).
/// Returns `None` if the logical id was never registered — callers must **not**
/// fall back to APIC 0 (that would kick the BSP by mistake).
pub(super) fn logical_to_apic(logical: usize) -> Option<u32> {
    if logical >= 64 {
        return None;
    }
    if LOGICAL_REGISTERED.load(Ordering::Acquire) & (1u64 << logical) == 0 {
        return None;
    }
    Some(LOGICAL_TO_APIC[logical].load(Ordering::Acquire))
}

// ─── ACPI handler ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AcpiMap;

impl AcpiHandler for AcpiMap {
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> PhysicalMapping<Self, T> {
        let aligned_start = physical_address & !(PAGE_SIZE - 1);
        let aligned_end = (physical_address + size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        PhysicalMapping::new(
            physical_address,
            NonNull::new_unchecked(phys_to_virt(physical_address) as *mut T),
            size,
            aligned_end - aligned_start,
            self.clone(),
        )
    }

    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {}
}

// ─── Timing ──────────────────────────────────────────────────────────────────

fn delay_us(us: u64) {
    // TSC ticks per microsecond ≈ CPU base frequency in MHz.
    // Apply a 4 GHz floor so the INIT→SIPI gap and AP-online wait are never
    // too short on a fast CPU where CPUID isn't available.  cpu_frequency()
    // now returns the raw CPUID value (no global floor) so we add the floor
    // here explicitly — it is safe to wait *longer* than needed, but not shorter.
    let ticks_per_us = crate::cpu::cpu_frequency().max(4000) as u64;
    let ticks = us.saturating_mul(ticks_per_us);
    let start = unsafe { core::arch::x86_64::_rdtsc() };
    while unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Start all APs found in the ACPI MADT.  Called once from BSP `primary_init()`.
pub fn start_application_processors() {
    // The BSP is always logical CPU 0. Register it before anything else so its
    // apic->logical mapping is in place even on a uniprocessor (early-return) path.
    register_cpu(raw_apic_id());

    // Master gate: unless `smp=on` was on the cmdline, boot single-core. AP
    // bring-up currently wedges some real hardware at the scheduler hand-off
    // (100% boot, no further progress); single-core is solid there. Defaulting
    // off keeps a stock image always-bootable while that hang is root-caused.
    // The BSP is already registered above, so single-core is fully functional.
    if !crate::common::ipi::smp_enabled() {
        warn!("[smp] AP bring-up disabled — single-core (pass `smp=on` on the cmdline to enable multi-core)");
        return;
    }

    let acpi_rsdp = KCONFIG.acpi_rsdp as usize;
    if acpi_rsdp == 0 {
        warn!("[smp] No ACPI RSDP — skipping AP startup");
        return;
    }

    let ap_lapic_ids = match enumerate_aps(acpi_rsdp) {
        Some(ids) => ids,
        None => return,
    };

    if ap_lapic_ids.is_empty() {
        warn!("[smp] no application processors in ACPI MADT");
        return;
    }

    warn!(
        "[smp] starting {} AP(s), LAPIC IDs: {:?}",
        ap_lapic_ids.len(),
        ap_lapic_ids
    );

    // Identity-map 0x6000..0x8000 so the trampoline can run after enabling paging.
    {
        use crate::vm::GenericPageTable;
        let mut pt = crate::vm::PageTable::from_current();
        let flags = MMUFlags::READ
            | MMUFlags::WRITE
            | MMUFlags::EXECUTE
            | MMUFlags::from_bits_truncate(CachePolicy::Cached as usize);
        if let Err(e) = pt.map_cont(TRAMPOLINE_PADDR, PAGE_SIZE * 2, TRAMPOLINE_PADDR, flags) {
            // `AlreadyMapped` is not necessarily fatal: on hardware whose boot
            // page tables cover low memory (rboot's physmap uses 2 MiB pages
            // there, so VA 0x6000 sits inside a live huge page), the mapping we
            // were about to create can already exist. What the trampoline needs
            // is only that these two pages translate to THEMSELVES (identity)
            // and are executable — verify that per page, upgrading flags in
            // place when EXEC/READ is missing, and reuse the mapping. Aborting
            // here cost every AP: the machine silently booted single-core
            // (observed on the RTX desktop as libinput's "your system is too
            // slow" — llvmpipe was rendering on one CPU).
            let need = MMUFlags::READ | MMUFlags::EXECUTE;
            let mut salvaged = matches!(e, crate::vm::PagingError::AlreadyMapped);
            if salvaged {
                for va in [TRAMPOLINE_PADDR, TRAMPOLINE_PADDR + PAGE_SIZE] {
                    match pt.query(va) {
                        Ok((pa, f, _)) if pa == va => {
                            if !f.contains(need) && pt.update(va, None, Some(flags)).is_err() {
                                salvaged = false;
                                break;
                            }
                        }
                        _ => {
                            salvaged = false;
                            break;
                        }
                    }
                }
            }
            if salvaged {
                crate::klog_info!(
                    "[smp] trampoline 0x6000..0x8000 already identity-mapped — reusing it"
                );
            } else {
                crate::klog_warn!(
                    "[smp] identity-map 0x6000 failed: {:?} — aborting AP startup \
                     (SIPI would triple-fault)",
                    e
                );
                core::mem::forget(pt);
                return;
            }
        }
        core::mem::forget(pt);
    }

    // Copy trampoline to physical 0x6000.
    unsafe { install_trampoline() };

    // Write BSP's CR3 and entry function. 32-bit trampoline loads CR3 before
    // long mode, so the PML4 physical address must fit in u32 (< 4 GiB).
    let cr3_phys = unsafe { cr3() } as u64;
    if cr3_phys > u32::MAX as u64 {
        crate::klog_warn!(
            "[smp] BSP CR3 {:#x} above 4GiB — AP trampoline cannot load it; \
             aborting AP startup",
            cr3_phys
        );
        return;
    }
    unsafe {
        (phys_to_virt(SLOT_CR3) as *mut u32).write_volatile(cr3_phys as u32);
        (phys_to_virt(SLOT_ENTRY) as *mut usize).write_volatile(KCONFIG.ap_fn as usize);
    }
    // Publish trampoline + slots before SIPI.
    core::sync::atomic::fence(Ordering::SeqCst);

    // Bring every AP up *concurrently*: send each one's INIT/SIPI and wait only
    // for it to latch its trampoline slot (the one hard serialization point — the
    // slots are shared and reused), then immediately move to the next AP. We
    // deliberately do NOT wait for each AP to finish its per-CPU init before
    // starting the next, for two reasons:
    //
    //   * It is safe. The two cross-AP hazards that once forced serialization are
    //     gone structurally — the AP-boot logical-id override is keyed by hardware
    //     APIC id (kernel-sync `AP_BOOT_LOGICAL`), and the GDT-extend step takes
    //     `GDT_EXTEND_LOCK`, so concurrent `init_ap`s no longer race.
    //
    //   * Serial per-AP online waits are pathological under single-threaded QEMU
    //     TCG. The BSP's online wait is an rdtsc busy-spin that competes with the
    //     very AP it is waiting on for the single host thread, so that AP's ~50 ms
    //     of real init dilates to ~10 s — it was seen coming online *68 ms* after
    //     a 10 s cap, and the old code then aborted every remaining AP (booting
    //     2 of 4 cores). Launching them together lets them race to online in one
    //     shared window instead of paying that tax once per AP.
    let mut latched = 0usize;
    for (idx, &lapic_id) in ap_lapic_ids.iter().enumerate() {
        if idx >= MAX_APS {
            crate::klog_warn!(
                "[smp] Too many APs (max {}), skipping LAPIC {}",
                MAX_APS,
                lapic_id
            );
            break;
        }

        // Assign this AP its dense logical id *before* it starts running, so the
        // very first lock it takes resolves to the right per-CPU slot.
        let logical = register_cpu(lapic_id);

        let stack_top = match alloc_ap_stack() {
            Some(top) => top,
            None => {
                crate::klog_warn!("[smp] failed to allocate stack for LAPIC {}", lapic_id);
                unregister_cpu(logical);
                break;
            }
        };
        // Arm the slot-consumed handshake *before* publishing this AP's slots
        // and sending the SIPI, so we can't miss an early ack.
        AP_SLOT_CONSUMED.store(false, Ordering::Release);
        unsafe { (phys_to_virt(SLOT_STACK) as *mut usize).write_volatile(stack_top) };
        unsafe {
            (phys_to_virt(SLOT_LOGICAL) as *mut u8).write_volatile(logical as u8);
        }

        crate::klog_info!(
            "[smp] Starting AP LAPIC {} (logical CPU {}) stack={:#x}",
            lapic_id,
            logical,
            stack_top
        );

        // INIT IPI → wait 10 ms → SIPI × 2
        zcore_drivers::irq::x86::Apic::send_init_ipi(lapic_id);
        delay_us(10_000);
        zcore_drivers::irq::x86::Apic::send_sipi(SIPI_VECTOR, lapic_id);
        delay_us(200);
        zcore_drivers::irq::x86::Apic::send_sipi(SIPI_VECTOR, lapic_id);
        delay_us(200);

        // CRITICAL: wait for this AP to *latch* its logical id out of the shared
        // trampoline slots before we reuse them for the next AP. The AP signals
        // this very early (right after copying the slot, before its slow per-CPU
        // init), so this is quick in the common case; the long cap only guards
        // against an AP that never started. Reusing the slots early makes two
        // APs share a logical id and silently corrupt each other's per-CPU state.
        let mut consumed = false;
        for _ in 0..2_000 {
            if AP_SLOT_CONSUMED.load(Ordering::Acquire) {
                consumed = true;
                break;
            }
            delay_us(1_000);
        }
        if !consumed {
            crate::klog_warn!(
                "[smp] AP LAPIC {} (logical {}) never latched its slot — skipping rest",
                lapic_id,
                logical
            );
            // The AP never picked up its slot; do not start more APs, as the
            // dead AP may still wake up later and read whatever we write next.
            break;
        }
        latched += 1;
    }

    // One best-effort barrier for *all* the APs we launched, instead of one per
    // AP. Each AP increments `AP_ONLINE_COUNT` (and marks itself in the online
    // bitmask) at the end of `secondary_init`, before it parks waiting for the
    // kernel to release it. Give them a bounded window to get there so the online
    // set is populated before cross-CPU TLB shootdowns can begin — but do not
    // block boot on it: the IPI and scheduler layers already tolerate an AP that
    // joins later (shootdowns only ever target CPUs already marked online, and a
    // not-yet-online AP holds no user TLB entries), so a straggler under
    // single-threaded TCG finishes coming online during the disk/init I/O that
    // follows, which naturally yields it the host thread.
    for _ in 0..AP_ONLINE_WAIT_MS {
        if AP_ONLINE_COUNT.load(Ordering::Acquire) >= latched {
            break;
        }
        delay_us(1_000);
    }

    let online = AP_ONLINE_COUNT.load(Ordering::Acquire);
    warn!(
        "[smp] done — {}/{} AP(s) online (launched {})",
        online,
        ap_lapic_ids.len(),
        latched
    );
}

/// Called by each AP from `secondary_init()` to announce it is running.
pub fn ap_signal_online() {
    // Mark this AP online so cross-CPU TLB shootdowns wait for it (and only
    // CPUs that actually came up — partial bring-up must not hang the waiter).
    crate::common::ipi::mark_cpu_online(super::cpu::cpu_id() as usize);
    AP_ONLINE_COUNT.fetch_add(1, Ordering::Release);
}

/// Dense logical CPU id written by the BSP for the AP currently being started.
pub fn ap_trampoline_logical_id() -> u8 {
    unsafe { (phys_to_virt(SLOT_LOGICAL) as *const u8).read_volatile() }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

unsafe fn install_trampoline() {
    let src = ap_trampoline_start as *const u8;
    let end = ap_trampoline_end as *const u8;
    let len = end.offset_from(src) as usize;
    let dst = phys_to_virt(TRAMPOLINE_PADDR) as *mut u8;
    core::ptr::copy_nonoverlapping(src, dst, len);
}

fn enumerate_aps(acpi_rsdp: usize) -> Option<Vec<u32>> {
    let tables = match unsafe { AcpiTables::from_rsdp(AcpiMap, acpi_rsdp) } {
        Ok(t) => t,
        Err(e) => {
            crate::klog_warn!("[smp] ACPI parse failed: {:?}", e);
            return None;
        }
    };

    let info = match tables.platform_info() {
        Ok(i) => i,
        Err(e) => {
            crate::klog_warn!("[smp] ACPI platform_info failed: {:?}", e);
            return None;
        }
    };

    let proc_info = match info.processor_info {
        Some(p) => p,
        None => {
            crate::klog_warn!("[smp] ACPI: no processor info in MADT");
            return None;
        }
    };

    use acpi::platform::ProcessorState;
    let ids: Vec<u32> = proc_info
        .application_processors
        .iter()
        .filter(|p| p.state != ProcessorState::Disabled)
        .map(|p| p.local_apic_id)
        .collect();

    Some(ids)
}

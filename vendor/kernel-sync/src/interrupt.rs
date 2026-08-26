use core::cell::UnsafeCell;

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "none", any(target_arch = "riscv32", target_arch = "riscv64")))] {
        mod interrupts {
            use core::sync::atomic::{AtomicU8, Ordering};
            use riscv::register::sstatus;

            /// Maps a hardware hart id (in `tp`, possibly sparse — e.g. boards that
            /// reserve hart 0) to a dense logical CPU id (0..NCPU). Populated by the
            /// HAL during SMP bring-up via [`set_logical_cpu_id`]; reads 0 until then
            /// (correct, since only the boot hart = logical 0 runs that early).
            static HARTID_TO_LOGICAL: [AtomicU8; 256] = {
                const ZERO: AtomicU8 = AtomicU8::new(0);
                [ZERO; 256]
            };

            /// Raw hart id of the current CPU (kernel convention: stored in `tp`).
            fn raw_hart_id() -> u8 {
                let hart_id: usize;
                unsafe {
                    core::arch::asm!("mv {0}, tp", out(reg) hart_id);
                }
                hart_id as u8
            }

            /// Register the logical id assigned to a given hart id.
            pub fn set_logical_cpu_id(hart_id: u32, logical_id: u8) {
                if let Some(slot) = HARTID_TO_LOGICAL.get(hart_id as usize) {
                    slot.store(logical_id, Ordering::Release);
                }
            }

            pub(crate) fn cpu_id() -> u8 {
                HARTID_TO_LOGICAL[raw_hart_id() as usize].load(Ordering::Acquire)
            }
            pub(crate) fn intr_on() {
                unsafe { sstatus::set_sie() };
            }
            pub(crate) fn intr_off() {
                unsafe { sstatus::clear_sie() };
            }
            pub(crate) fn intr_get() -> bool {
                sstatus::read().sie()
            }
        }
    } else if #[cfg(all(target_os = "none", any(target_arch = "x86", target_arch = "x86_64")))] {
        mod interrupts {
            use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
            use x86_64::instructions::interrupts;

            use crate::MAX_CORE_NUM;

            /// `IA32_APIC_BASE`. Bit 11 = APIC global enable, bit 10 = x2APIC mode.
            const IA32_APIC_BASE: u32 = 0x1B;
            const APIC_BASE_ENABLE: u64 = 1 << 11;
            const APIC_BASE_EXTD: u64 = 1 << 10;
            /// `IA32_X2APIC_APICID` — the APIC ID register in x2APIC mode.
            const IA32_X2APIC_APICID: u32 = 0x802;

            /// Maps a hardware Local APIC ID that fits in a byte to a dense logical
            /// CPU id (0..NCPU). APIC IDs are *not* contiguous on real hardware
            /// (cores/threads/sockets leave gaps), so using them directly to index
            /// per-CPU arrays causes out-of-bounds panics. The table is populated by
            /// the HAL during SMP bring-up via [`set_logical_cpu_id`]. Until then it
            /// reads 0, which is correct because only the BSP (logical 0) runs before
            /// the APs are enumerated.
            static APIC_TO_LOGICAL: [AtomicU8; 256] = {
                const ZERO: AtomicU8 = AtomicU8::new(0);
                [ZERO; 256]
            };

            /// Reverse map, indexed by the *dense logical* id: the hardware APIC ID
            /// of each registered CPU. Needed because x2APIC IDs are 32-bit and can
            /// exceed 255, which the byte-indexed table above cannot represent —
            /// two such CPUs would otherwise alias onto one logical id and silently
            /// share a per-CPU slot. `APIC_ID_VALID` marks the populated entries
            /// (APIC ID 0 is a legal BSP id, so 0 cannot mean "unset").
            static APIC_ID_OF_LOGICAL: [AtomicU32; MAX_CORE_NUM] =
                [const { AtomicU32::new(0) }; MAX_CORE_NUM];
            static APIC_ID_VALID: AtomicU64 = AtomicU64::new(0);

            /// `phys + offset` virtual mapping for the LAPIC MMIO page (set by HAL at boot).
            static PHYS_VIRT_OFFSET: AtomicU64 = AtomicU64::new(0);

            /// Register the kernel's phys→virt linear map offset (from UEFI/boot config).
            pub fn set_phys_virt_offset(offset: u64) {
                PHYS_VIRT_OFFSET.store(offset, Ordering::Release);
            }

            /// Whether this CPU's Local APIC is in x2APIC mode.
            ///
            /// Load-bearing: in x2APIC mode the LAPIC **stops decoding its MMIO
            /// page**, so the register window used below reads whatever the
            /// (unclaimed) bus returns — typically all-ones. Every APIC register
            /// must go through the MSR interface once this is set.
            fn x2apic_active() -> bool {
                let base = unsafe { x86_64::registers::model_specific::Msr::new(IA32_APIC_BASE).read() };
                base & (APIC_BASE_ENABLE | APIC_BASE_EXTD) == (APIC_BASE_ENABLE | APIC_BASE_EXTD)
            }

            /// Read the Local APIC ID from the MMIO register (xAPIC mode only).
            fn read_lapic_id_mmio() -> Option<u32> {
                use x86_64::registers::model_specific::Msr;
                let offset = PHYS_VIRT_OFFSET.load(Ordering::Acquire);
                if offset == 0 {
                    return None;
                }
                let base = unsafe { Msr::new(IA32_APIC_BASE).read() };
                if base & APIC_BASE_ENABLE == 0 || base & APIC_BASE_EXTD != 0 {
                    // Disabled, or x2APIC: the MMIO window is not readable.
                    return None;
                }
                let page_phys = (base & 0xFFFF_F000) as u64;
                let id_ptr = (page_phys.wrapping_add(offset) + 0x20) as *const u32;
                let id_reg = unsafe { core::ptr::read_volatile(id_ptr) };
                // xAPIC keeps the id in bits 31:24 of the ID register.
                Some(id_reg >> 24)
            }

            /// Initial APIC ID from CPUID, used when the LAPIC itself cannot be
            /// queried yet. Leaf 0x0B (x2APIC topology) reports the full 32-bit
            /// id; the legacy leaf 1 field is only 8 bits wide.
            fn cpuid_apic_id() -> u32 {
                use core::arch::x86_64::{__cpuid, __cpuid_count};
                if __cpuid(0).eax >= 0x0B {
                    let leaf = __cpuid_count(0x0B, 0);
                    // EBX[15:0] == 0 means the leaf is not valid on this CPU.
                    if leaf.ebx & 0xFFFF != 0 {
                        return leaf.edx;
                    }
                }
                __cpuid(1).ebx >> 24
            }

            /// Raw Local APIC ID of the current CPU (hardware id, sparse and — in
            /// x2APIC mode — up to 32 bits wide).
            pub(super) fn raw_apic_id() -> u32 {
                if x2apic_active() {
                    return unsafe {
                        x86_64::registers::model_specific::Msr::new(IA32_X2APIC_APICID).read() as u32
                    };
                }
                read_lapic_id_mmio().unwrap_or_else(cpuid_apic_id)
            }

            /// Register the logical id assigned to a given Local APIC ID. Called once
            /// per CPU from the HAL before that CPU starts executing kernel code.
            pub fn set_logical_cpu_id(apic_id: u32, logical_id: u8) {
                if (logical_id as usize) < MAX_CORE_NUM {
                    APIC_ID_OF_LOGICAL[logical_id as usize].store(apic_id, Ordering::Release);
                    APIC_ID_VALID.fetch_or(1u64 << logical_id, Ordering::Release);
                }
                if apic_id < 256 {
                    APIC_TO_LOGICAL[apic_id as usize].store(logical_id, Ordering::Release);
                }
            }

            /// Resolve a hardware APIC ID to its dense logical id. Byte-sized ids
            /// hit the direct table; wider (x2APIC) ids scan the registered set,
            /// which is at most `MAX_CORE_NUM` entries and only reached on the
            /// pre-GS fallback path.
            fn apic_to_logical(apic: u32) -> u8 {
                if apic < 256 {
                    return APIC_TO_LOGICAL[apic as usize].load(Ordering::Acquire);
                }
                let mut valid = APIC_ID_VALID.load(Ordering::Acquire);
                while valid != 0 {
                    let logical = valid.trailing_zeros() as usize;
                    valid &= valid - 1;
                    if APIC_ID_OF_LOGICAL[logical].load(Ordering::Acquire) == apic {
                        return logical as u8;
                    }
                }
                0
            }

            /// Dense logical id override while an AP runs [`init_ap`] (GS not
            /// ready yet). Indexed by the **dense logical id** — unique per AP by
            /// construction — and looked up by hardware APIC id, so two APs
            /// booting concurrently can never clobber each other's override even
            /// if their APIC ids do not fit in a byte.
            static AP_BOOT_APIC: [AtomicU32; MAX_CORE_NUM] =
                [const { AtomicU32::new(u32::MAX) }; MAX_CORE_NUM];
            /// Bitmask of logical ids currently inside a [`with_ap_boot_logical`]
            /// window. Zero on the steady-state path, which lets [`cpu_id`] skip
            /// the (expensive) APIC-id read entirely.
            static AP_BOOT_ACTIVE: AtomicU64 = AtomicU64::new(0);

            pub fn with_ap_boot_logical<R>(logical: u8, f: impl FnOnce() -> R) -> R {
                let idx = logical as usize;
                if idx >= MAX_CORE_NUM {
                    return f();
                }
                AP_BOOT_APIC[idx].store(raw_apic_id(), Ordering::Release);
                AP_BOOT_ACTIVE.fetch_or(1u64 << idx, Ordering::Release);
                let ret = f();
                AP_BOOT_ACTIVE.fetch_and(!(1u64 << idx), Ordering::Release);
                AP_BOOT_APIC[idx].store(u32::MAX, Ordering::Release);
                ret
            }

            /// The logical id claimed by an AP currently inside its `init_ap`
            /// window, if the calling CPU is that AP.
            fn ap_boot_logical() -> Option<u8> {
                let mut active = AP_BOOT_ACTIVE.load(Ordering::Acquire);
                if active == 0 {
                    return None;
                }
                let apic = raw_apic_id();
                while active != 0 {
                    let logical = active.trailing_zeros() as usize;
                    active &= active - 1;
                    if AP_BOOT_APIC[logical].load(Ordering::Acquire) == apic {
                        return Some(logical as u8);
                    }
                }
                None
            }

            pub(crate) fn cpu_id() -> u8 {
                // Prefer the AP-boot override BEFORE touching GS: during
                // `init_ap`, GSBASE is still 0 and `logical_cpu_id_valid()`
                // would read linear address ~0 (null-guard #PF or false id).
                //
                // The relaxed mask load short-circuits this in the steady state.
                // It matters: `cpu_id()` runs on every `push_off`/`pop_off`, i.e.
                // on every kernel lock acquire and release, and resolving an APIC
                // id costs an RDMSR plus (in xAPIC mode) an uncached MMIO read.
                if AP_BOOT_ACTIVE.load(Ordering::Relaxed) != 0 {
                    if let Some(logical) = ap_boot_logical() {
                        return logical;
                    }
                }
                #[cfg(target_arch = "x86_64")]
                {
                    if trapframe::logical_cpu_id_valid() {
                        return trapframe::read_logical_cpu_id();
                    }
                }
                apic_to_logical(raw_apic_id())
            }
            pub(crate) fn intr_on() {
                interrupts::enable();
            }
            pub(crate) fn intr_off() {
                interrupts::disable();
            }
            pub(crate) fn intr_get() -> bool {
                interrupts::are_enabled()
            }
        }
    } else if #[cfg(all(target_os = "none", target_arch = "aarch64"))] {
        mod interrupts {
            pub(crate) fn cpu_id() -> u8 {
                // Dense logical id, written to TPIDR_EL1 by the kernel per CPU.
                // MPIDR affinity is sparse across clusters (Aff0 repeats), so it
                // can't index per-CPU arrays; TPIDR_EL1 holds the logical id
                // directly (0 on the boot CPU until secondaries are brought up).
                let id: u64;
                unsafe { core::arch::asm!("mrs {0}, tpidr_el1", out(reg) id) };
                id as u8
            }
            pub(crate) fn intr_on() {
                unsafe {
                    core::arch::asm!("msr daifclr, #2");
                }
            }
            pub(crate) fn intr_off() {
                unsafe {
                    core::arch::asm!("msr daifset, #2");
                }
            }
            pub(crate) fn intr_get() -> bool {
                use cortex_a::registers::DAIF;
                use tock_registers::interfaces::Readable;
                !DAIF.is_set(DAIF::I)
            }
        }
    } else {
        mod interrupts {
            pub(crate) fn cpu_id() -> u8 {
                unimplemented!();
            }
            pub(crate) fn intr_on() { unimplemented!(); }
            pub(crate) fn intr_off() { unimplemented!(); }
            pub(crate) fn intr_get() -> bool {
                unimplemented!();
            }
        }
    }
}

use interrupts::*;

/// Current CPU's dense logical id (0..NCPU).
///
/// On x86 this resolves the sparse Local APIC ID through the table populated by
/// [`set_logical_cpu_id`]; on riscv/aarch64 the architecture already provides a
/// dense id (hart id / MPIDR affinity).
pub fn current_cpu_id() -> u8 {
    cpu_id()
}

/// Raw hardware Local APIC ID (x86). Sparse, and up to 32 bits wide in x2APIC
/// mode; use [`current_cpu_id`] to index arrays.
#[cfg(all(target_os = "none", any(target_arch = "x86", target_arch = "x86_64")))]
pub fn hardware_apic_id() -> u32 {
    interrupts::raw_apic_id()
}

/// Register the dense logical id assigned to a hardware CPU id (Local APIC ID on
/// x86, hart id on riscv).
///
/// Must be called once per CPU (including the BSP) before that CPU executes any
/// code that takes a lock, so that `cpu_id()` never returns a stale/colliding id.
#[cfg(all(
    target_os = "none",
    any(
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "riscv32",
        target_arch = "riscv64"
    )
))]
pub fn set_logical_cpu_id(hw_id: u32, logical_id: u8) {
    interrupts::set_logical_cpu_id(hw_id, logical_id)
}

/// Register phys→virt linear map offset for LAPIC MMIO reads on x86.
#[cfg(all(target_os = "none", any(target_arch = "x86", target_arch = "x86_64")))]
pub fn set_phys_virt_offset(offset: u64) {
    interrupts::set_phys_virt_offset(offset)
}

/// Run `f` while [`cpu_id`] returns `logical` (AP [`init_ap`] before GS is ready).
#[cfg(all(target_os = "none", any(target_arch = "x86", target_arch = "x86_64")))]
pub fn with_ap_boot_logical<R>(logical: u8, f: impl FnOnce() -> R) -> R {
    interrupts::with_ap_boot_logical(logical, f)
}

#[derive(Debug, Default, Clone, Copy)]
#[repr(align(64))]
pub struct Cpu {
    pub noff: i32,              // Depth of push_off() nesting.
    pub interrupt_enable: bool, // Were interrupts enabled before push_off()?
}

impl Cpu {
    const fn new() -> Self {
        Self {
            noff: 0,
            interrupt_enable: false,
        }
    }
}

pub struct CpuStorage(UnsafeCell<Cpu>);

// SAFETY: each CPU only ever accesses CPUS[cpu_id()]; wrong ids are fixed at AP boot.
unsafe impl Sync for CpuStorage {}

impl CpuStorage {
    const fn new() -> Self {
        Self(UnsafeCell::new(Cpu::new()))
    }

    // Deliberate: hands out a `&mut Cpu` view of an `UnsafeCell` from `&self`.
    // Each slot is per-CPU and only touched by its owning core, so the standard
    // `mut_from_ref` guard does not apply.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    fn get(&self) -> &mut Cpu {
        // SAFETY: caller ensures this slot is owned by the current CPU.
        unsafe { &mut *self.0.get() }
    }
}

// Avoid hard code
#[allow(clippy::declare_interior_mutable_const)]
const DEFAULT_CPU: CpuStorage = CpuStorage::new();

// Tamaño único de los arrays per-CPU del sistema (id lógico denso); lo
// reutilizan el scheduler (vendor/PreemptiveScheduler) y kernel-hal lo
// verifica en compilación contra su `config::MAX_CORE_NUM`.
use crate::MAX_CORE_NUM;

static CPUS: [CpuStorage; MAX_CORE_NUM] = [DEFAULT_CPU; MAX_CORE_NUM];

#[inline]
pub fn mycpu() -> &'static mut Cpu {
    let id = cpu_id() as usize;
    assert!(id < MAX_CORE_NUM, "cpu_id {} >= MAX_CORE_NUM", id);
    CPUS[id].get()
}

/// How many kernel lock guards this CPU currently holds.
///
/// Every guard handed out by this crate (`Mutex`, `TicketMutex`, `RwLock`, …)
/// brackets its lifetime with `push_off`/`pop_off`, so `noff == 0` means the
/// core holds **no** kernel lock: nothing is half-updated behind a lock and no
/// later acquisition of one can deadlock against this context. That is the
/// precondition the panic-recovery path (`zcore::oops`) tests before it dares
/// to run recovery code — which itself takes locks — from inside a panic.
///
/// Unlike [`mycpu`] this never asserts: it is called from the panic handler,
/// where a second panic (out-of-range cpu id) would abort the machine. An
/// unknown cpu id reports "locks held", i.e. the conservative answer.
#[inline]
pub fn lock_depth() -> i32 {
    let id = cpu_id() as usize;
    if id >= MAX_CORE_NUM {
        return i32::MAX;
    }
    CPUS[id].get().noff
}

// push_off/pop_off are like intr_off()/intr_on() except that they are matched:
// it takes two pop_off()s to undo two push_off()s.  Also, if interrupts
// are initially off, then push_off, pop_off leaves them off.
pub(crate) fn push_off() {
    let old = intr_get();
    intr_off();
    let mut cpu = mycpu();
    if cpu.noff == 0 {
        cpu.interrupt_enable = old;
    }
    cpu.noff += 1;
}

pub(crate) fn pop_off() {
    let mut cpu = mycpu();
    if intr_get() || cpu.noff < 1 {
        panic!("pop_off");
    }
    cpu.noff -= 1;
    let should_enable = cpu.noff == 0 && cpu.interrupt_enable;
    drop(cpu);
    // NOTICE: intr_on() may lead to an immediate inerrupt, so we *MUST* drop(cpu) in advance.
    if should_enable {
        intr_on();
    }
}

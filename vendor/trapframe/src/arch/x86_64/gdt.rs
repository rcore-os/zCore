//! Configure Global Descriptor Table (GDT)

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::arch::asm;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use x86_64::instructions::tables::{lgdt, load_tss};
use x86_64::registers::model_specific::{GsBase, KernelGsBase, Star};
use x86_64::structures::gdt::{Descriptor, SegmentSelector};
use x86_64::structures::DescriptorTablePointer;
use x86_64::{PrivilegeLevel, VirtAddr};

/// Base address and entry count of the GDT created by the BSP's [`init`].
/// APs read this instead of their own `sgdt()` (which points at the trampoline's
/// 3-entry mini-GDT and is therefore missing all user-space segments).
static BSP_GDT_BASE: AtomicU64 = AtomicU64::new(0);
static BSP_GDT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The `STAR` MSR value written by the BSP.  `STAR` is a per-CPU MSR, so each
/// AP must write it independently with the same value.
/// Packed as `(u_cs_raw << 16) | k_cs_raw` into a single AtomicU32.
static BSP_STAR: AtomicU32 = AtomicU32::new(0);

#[cfg(not(feature = "ioport_bitmap"))]
type TSS = x86_64::structures::tss::TaskStateSegment;
#[cfg(feature = "ioport_bitmap")]
type TSS = super::ioport::TSSWithPortBitmap;

/// The region pointed to by `GSBASE` on each CPU.
///
/// `GSBASE` (and the TSS descriptor) point at the `tss` field, which therefore
/// MUST stay first: the syscall/trap entry stubs read `gs:4` / `gs:12` as
/// offsets *into the TSS*. The trailing `cpu_local` word is appended storage for
/// a kernel per-CPU pointer (à la Redox's ProcessorControlRegion), read/written
/// via `gs:[offset_of!(.., cpu_local)]`. Reads through GSBASE are not bounded by
/// the TSS segment limit, so the extra word is always reachable.
#[repr(C)]
struct CpuLocalRegion {
    tss: TSS,
    cpu_local: usize,
    /// Dense logical CPU id (0..NCPU), valid once [`write_logical_cpu_id`] runs.
    logical_cpu_id: u8,
    logical_cpu_valid: u8,
    _pad: [u8; 6],
    /// DEBUG: dirección base donde `trap_syscall_entry` guardó el último
    /// `GeneralRegs` en ESTA CPU (escrita por el asm vía `gs:`).
    dbg_save: usize,
}

/// DEBUG: offset de `dbg_save` dentro de [`CpuLocalRegion`], para el asm.
pub const DBG_SAVE_GS_OFFSET: usize = core::mem::offset_of!(CpuLocalRegion, dbg_save);

/// DEBUG: leer la dirección de save per-CPU registrada por el asm.
#[inline]
pub fn read_dbg_save() -> usize {
    let ret: usize;
    unsafe {
        asm!(
            "mov {ret}, gs:[{off}]",
            ret = out(reg) ret,
            off = const core::mem::offset_of!(CpuLocalRegion, dbg_save),
            options(nostack, preserves_flags, readonly),
        );
    }
    ret
}

/// Read the kernel per-CPU pointer stored in this CPU's GS region.
///
/// Returns 0 if [`init`] has not run yet on this CPU.
#[inline]
pub fn read_cpu_local() -> usize {
    let ret: usize;
    unsafe {
        asm!(
            "mov {ret}, gs:[{off}]",
            ret = out(reg) ret,
            off = const core::mem::offset_of!(CpuLocalRegion, cpu_local),
            options(nostack, preserves_flags, readonly),
        );
    }
    ret
}

/// Store a kernel per-CPU pointer in this CPU's GS region.
///
/// # Safety
///
/// `GSBASE` must point to a [`CpuLocalRegion`], i.e. [`init`] must have run on
/// the current CPU.
#[inline]
pub unsafe fn write_cpu_local(val: usize) {
    asm!(
        "mov gs:[{off}], {val}",
        val = in(reg) val,
        off = const core::mem::offset_of!(CpuLocalRegion, cpu_local),
        options(nostack, preserves_flags),
    );
}

/// Whether this CPU's dense logical id has been stored in the GS region.
#[inline]
pub fn logical_cpu_id_valid() -> bool {
    let valid: u8;
    unsafe {
        asm!(
            "mov {valid}, gs:[{off}]",
            valid = out(reg_byte) valid,
            off = const core::mem::offset_of!(CpuLocalRegion, logical_cpu_valid),
            options(nostack, preserves_flags, readonly),
        );
    }
    valid != 0
}

/// Read the dense logical CPU id from the GS region.
///
/// # Panics
///
/// If [`logical_cpu_id_valid`] is false.
#[inline]
pub fn read_logical_cpu_id() -> u8 {
    let id: u8;
    unsafe {
        asm!(
            "mov {id}, gs:[{off}]",
            id = out(reg_byte) id,
            off = const core::mem::offset_of!(CpuLocalRegion, logical_cpu_id),
            options(nostack, preserves_flags, readonly),
        );
    }
    id
}

/// Store the dense logical CPU id for this CPU in the GS region.
///
/// # Safety
///
/// [`init`] must have run on the current CPU.
#[inline]
pub unsafe fn write_logical_cpu_id(id: u8) {
    asm!(
        "mov gs:[{id_off}], {id}",
        "mov byte ptr gs:[{valid_off}], 1",
        id = in(reg_byte) id,
        id_off = const core::mem::offset_of!(CpuLocalRegion, logical_cpu_id),
        valid_off = const core::mem::offset_of!(CpuLocalRegion, logical_cpu_valid),
        options(nostack, preserves_flags),
    );
}

/// Init TSS & GDT.
pub fn init() {
    // allocate stack for trap from user
    // set the stack top to TSS
    // so that when trap from ring3 to ring0, CPU can switch stack correctly
    let mut region = Box::new(CpuLocalRegion {
        tss: TSS::new(),
        cpu_local: 0,
        logical_cpu_id: 0,
        logical_cpu_valid: 0,
        _pad: [0; 6],
        dbg_save: 0,
    });
    // Trap/exception stack used on ring3->ring0 transitions (TSS sp0). 4 KiB was
    // dangerously small for deep user-trap handling (page-fault -> VMO commit ->
    // smoltcp ...): an overflow silently corrupts the adjacent heap (observed as
    // wild UserContext corruption -> iret to ring-0 junk -> #UD). Use 64 KiB.
    let trap_stack_top = Box::leak(Box::new([0u8; 0x10000])).as_ptr() as u64 + 0x10000;
    region.tss.privilege_stack_table[0] = VirtAddr::new(trap_stack_top);
    // Dedicated #DF stack (IST1; see idt.rs vector 8). A double fault caused
    // by a kernel stack overflow cannot be handled on the faulting stack —
    // the CPU must switch to a known-good one or the machine triple-faults
    // silently. 16 KiB is ample: the #DF handler only prints and panics.
    let df_stack_top = Box::leak(Box::new([0u8; 0x4000])).as_ptr() as u64 + 0x4000;
    region.tss.interrupt_stack_table[0] = VirtAddr::new(df_stack_top);
    // Dedicated #GP stack (IST2; see idt.rs vector 13). A #GP raised because an
    // interrupt/`ret` landed on a corrupted kernel stack cannot push its
    // exception frame onto that same stack — it faults again and escalates
    // #GP -> #DF -> triple fault (the machine dies silently). Giving #GP its own
    // known-good stack lets the handler run: it repairs a mangled saved RIP and
    // resumes, or panics with a backtrace instead of vanishing. 16 KiB is ample.
    let gp_stack_top = Box::leak(Box::new([0u8; 0x4000])).as_ptr() as u64 + 0x4000;
    region.tss.interrupt_stack_table[1] = VirtAddr::new(gp_stack_top);
    let region: &'static CpuLocalRegion = Box::leak(region);
    let tss: &'static TSS = &region.tss;
    let (tss0, tss1) = match Descriptor::tss_segment(tss) {
        Descriptor::SystemSegment(tss0, tss1) => (tss0, tss1),
        _ => unreachable!(),
    };
    // Extreme hack: the segment limit assumed by x86_64 does not include the port bitmap.
    #[cfg(feature = "ioport_bitmap")]
    let tss0 = (tss0 & !0xFFFF) | (size_of::<TSS>() as u64);

    unsafe {
        // get current GDT
        let gdtp = sgdt();
        let entry_count = (gdtp.limit + 1) as usize / size_of::<u64>();
        let old_gdt = core::slice::from_raw_parts(gdtp.base.as_ptr::<u64>(), entry_count);

        // allocate new GDT with 7 more entries
        //
        // NOTICE: for fast syscall:
        //   STAR[47:32] = K_CS   = K_SS - 8
        //   STAR[63:48] = U_CS32 = U_SS32 - 8 = U_CS - 16
        let mut gdt = Vec::from(old_gdt);
        gdt.extend([tss0, tss1, KCODE64, KDATA64, UCODE32, UDATA32, UCODE64].iter());
        let gdt = Vec::leak(gdt);

        // load new GDT and TSS
        lgdt(&DescriptorTablePointer {
            limit: gdt.len() as u16 * 8 - 1,
            base: VirtAddr::new(gdt.as_ptr() as _),
        });
        load_tss(SegmentSelector::new(
            entry_count as u16,
            PrivilegeLevel::Ring0,
        ));

        // for fast syscall:
        // store address of TSS to kernel_gsbase
        //
        // `swapgs` (used by every trap/syscall entry and exit in trap.S /
        // syscall.S to reach `gs:4`/`gs:12`, i.e. this CpuLocalRegion's TSS
        // slots) EXCHANGES GsBase (IA32_GS_BASE, MSR 0xC0000101 -- the
        // currently active one) with KernelGsBase (IA32_KERNEL_GS_BASE, MSR
        // 0xC0000102). Writing only GsBase leaves KernelGsBase at its
        // power-up value (0, never otherwise written anywhere in this
        // codebase) -- so the very first `swapgs` executed on this cpu (on
        // its first trap taken from ring 3) swaps a *valid* GsBase for a
        // *zero* one, and every `gs:`-relative access in that entry's
        // trampoline prologue (`__from_user` in trap.S, `syscall_entry` in
        // syscall.S) resolves against linear address ~0 instead of this
        // region -- under whatever page table was active for the
        // interrupted user thread, that is normally the deliberately
        // unmapped null-guard page, so the very read/write that is supposed
        // to fetch this cpu's kernel stack (TSS.sp0) instead takes a second,
        // nested page fault while the first trap is still being delivered:
        // exactly x86's double-fault trigger condition. Writing the same
        // pointer to both MSRs makes `swapgs` idempotent with respect to the
        // value `gs:` accesses see, regardless of how many times it has
        // fired -- the standard pattern for any kernel that uses swapgs.
        #[allow(const_item_mutation)]
        GsBase::MSR.write(tss as *const _ as u64);
        #[allow(const_item_mutation)]
        KernelGsBase::MSR.write(tss as *const _ as u64);

        let star_k_cs = SegmentSelector::new(entry_count as u16 + 2, PrivilegeLevel::Ring0).0;
        let star_u_cs = SegmentSelector::new(entry_count as u16 + 4, PrivilegeLevel::Ring3).0;
        Star::write_raw(star_u_cs, star_k_cs);

        // Persist the BSP GDT info for APs.  APs cannot use their own sgdt()
        // after the trampoline because it points at a 3-entry mini-GDT that
        // lacks KCODE64/UCODE64/UDATA32 etc.  They must extend THIS GDT.
        BSP_GDT_BASE.store(gdt.as_ptr() as u64, Ordering::Release);
        BSP_GDT_COUNT.store(gdt.len(), Ordering::Release);
        // Pack STAR fields so APs can replicate the write.
        BSP_STAR.store(
            ((star_u_cs as u32) << 16) | star_k_cs as u32,
            Ordering::Release,
        );
    }
}

/// Per-CPU GDT/TSS/GSBASE setup for application processors.
///
/// The BSP must call [`init`] first.  Extends the BSP's GDT with this AP's
/// TSS entry, loads it, and points `GSBASE` at the AP's [`CpuLocalRegion`].
/// Also replicates the `STAR` MSR (segment selectors for syscall/sysret),
/// which is a per-CPU register that the BSP set during [`init`].
pub fn init_ap() {
    let mut region = Box::new(CpuLocalRegion {
        tss: TSS::new(),
        cpu_local: 0,
        logical_cpu_id: 0,
        logical_cpu_valid: 0,
        _pad: [0; 6],
        dbg_save: 0,
    });
    // Trap/exception stack used on ring3->ring0 transitions (TSS sp0). 4 KiB was
    // dangerously small for deep user-trap handling (page-fault -> VMO commit ->
    // smoltcp ...): an overflow silently corrupts the adjacent heap (observed as
    // wild UserContext corruption -> iret to ring-0 junk -> #UD). Use 64 KiB.
    let trap_stack_top = Box::leak(Box::new([0u8; 0x10000])).as_ptr() as u64 + 0x10000;
    region.tss.privilege_stack_table[0] = VirtAddr::new(trap_stack_top);
    // Dedicated #DF stack (IST1; see idt.rs vector 8). A double fault caused
    // by a kernel stack overflow cannot be handled on the faulting stack —
    // the CPU must switch to a known-good one or the machine triple-faults
    // silently. 16 KiB is ample: the #DF handler only prints and panics.
    let df_stack_top = Box::leak(Box::new([0u8; 0x4000])).as_ptr() as u64 + 0x4000;
    region.tss.interrupt_stack_table[0] = VirtAddr::new(df_stack_top);
    // Dedicated #GP stack (IST2; see idt.rs vector 13). A #GP raised because an
    // interrupt/`ret` landed on a corrupted kernel stack cannot push its
    // exception frame onto that same stack — it faults again and escalates
    // #GP -> #DF -> triple fault (the machine dies silently). Giving #GP its own
    // known-good stack lets the handler run: it repairs a mangled saved RIP and
    // resumes, or panics with a backtrace instead of vanishing. 16 KiB is ample.
    let gp_stack_top = Box::leak(Box::new([0u8; 0x4000])).as_ptr() as u64 + 0x4000;
    region.tss.interrupt_stack_table[1] = VirtAddr::new(gp_stack_top);
    let region: &'static CpuLocalRegion = Box::leak(region);
    let tss: &'static TSS = &region.tss;
    let (tss0, tss1) = match Descriptor::tss_segment(tss) {
        Descriptor::SystemSegment(tss0, tss1) => (tss0, tss1),
        _ => unreachable!(),
    };
    #[cfg(feature = "ioport_bitmap")]
    let tss0 = (tss0 & !0xFFFF) | (size_of::<TSS>() as u64);

    static GDT_EXTEND_LOCK: AtomicBool = AtomicBool::new(false);
    while GDT_EXTEND_LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }

    unsafe {
        // Read the BSP's full GDT (not sgdt() which would give the AP's own
        // 3-entry trampoline GDT, missing all user-space segment descriptors).
        let base = BSP_GDT_BASE.load(Ordering::Acquire);
        let entry_count = BSP_GDT_COUNT.load(Ordering::Acquire);
        assert!(base != 0, "BSP GDT not initialized before AP init_ap");
        let old_gdt = core::slice::from_raw_parts(base as *const u64, entry_count);
        let mut gdt = Vec::from(old_gdt);
        gdt.extend([tss0, tss1].iter());
        let gdt = Vec::leak(gdt);
        lgdt(&DescriptorTablePointer {
            limit: gdt.len() as u16 * 8 - 1,
            base: VirtAddr::new(gdt.as_ptr() as _),
        });

        // The AP entered via the trampoline with CS=0x8 (mini-GDT code segment).
        // After lgdt() the mini-GDT is no longer the active GDT, so CS=0x8 now
        // points at BSP's GDT[1] (a UEFI firmware descriptor, typically a 32-bit
        // code or data segment).  Any interrupt that fires before a far branch
        // will save CS=0x8 to the stack; on iretq the CPU reloads CS=0x8 from
        // the new GDT and faults with #GP error_code=0x8.  Fix: far-return to
        // immediately reload CS with KCODE64.
        //
        // BSP's init() appended [tss0, tss1, KCODE64, KDATA64, UCODE32, UDATA32,
        // UCODE64], so KCODE64 is at GDT index (original_count + 2).
        // entry_count = BSP_GDT_COUNT = original_count + 7, so
        // KCODE64 index = entry_count - 5.
        let kcode64_idx = entry_count as u16 - 5;
        let kcode64_sel = SegmentSelector::new(kcode64_idx, PrivilegeLevel::Ring0).0 as u64;
        asm!(
            // Push new CS, then the return address, then 64-bit far return.
            // 0x48 0xcb = REX.W RETF — the 64-bit far return (lretq).
            // This atomically reloads CS with kcode64_sel and continues at "2:".
            "pushq {sel}",
            "leaq 2f(%rip), {tmp}",
            "pushq {tmp}",
            ".byte 0x48, 0xcb",
            "2:",
            sel = in(reg) kcode64_sel,
            tmp = lateout(reg) _,
            options(att_syntax),
        );

        load_tss(SegmentSelector::new(
            entry_count as u16,
            PrivilegeLevel::Ring0,
        ));
        // See the matching comment in `init()`: both MSRs must hold this AP's
        // own CpuLocalRegion pointer, or this AP's first `swapgs` (on its
        // first trap from ring 3) hands `gs:`-relative accesses a zeroed
        // base instead of it.
        #[allow(const_item_mutation)]
        GsBase::MSR.write(tss as *const _ as u64);
        #[allow(const_item_mutation)]
        KernelGsBase::MSR.write(tss as *const _ as u64);

        // Replicate the STAR MSR (per-CPU) that the BSP set during init().
        let packed = BSP_STAR.load(Ordering::Acquire);
        let star_k_cs = (packed & 0xFFFF) as u16;
        let star_u_cs = (packed >> 16) as u16;
        Star::write_raw(star_u_cs, star_k_cs);
    }

    GDT_EXTEND_LOCK.store(false, Ordering::Release);
}

/// Get current GDT register
#[inline]
unsafe fn sgdt() -> DescriptorTablePointer {
    let mut gdt = DescriptorTablePointer {
        limit: 0,
        base: VirtAddr::zero(),
    };
    asm!("sgdt [{}]", in(reg) &mut gdt);
    gdt
}

const KCODE64: u64 = 0x00209800_00000000; // EXECUTABLE | USER_SEGMENT | PRESENT | LONG_MODE
const UCODE64: u64 = 0x0020F800_00000000; // EXECUTABLE | USER_SEGMENT | USER_MODE | PRESENT | LONG_MODE
const KDATA64: u64 = 0x00009200_00000000; // DATA_WRITABLE | USER_SEGMENT | PRESENT
#[allow(dead_code)]
const UDATA64: u64 = 0x0000F200_00000000; // DATA_WRITABLE | USER_SEGMENT | USER_MODE | PRESENT
const UCODE32: u64 = 0x00cffa00_0000ffff; // EXECUTABLE | USER_SEGMENT | USER_MODE | PRESENT
const UDATA32: u64 = 0x00cff200_0000ffff; // EXECUTABLE | USER_SEGMENT | USER_MODE | PRESENT

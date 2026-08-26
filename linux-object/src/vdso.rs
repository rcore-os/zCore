//! The kernel half of the Linux-compatible vDSO.
//!
//! `linux-vdso` holds the image and the code that runs inside it. This module
//! gives that image a home in physical memory, keeps the clock parameters
//! inside it current, and maps it into every Linux process — the three things
//! that turn a shared object into a working `clock_gettime`.
//!
//! ## Why the image lives in a physical VMO
//!
//! There is exactly one copy of the image in the whole system, and every
//! process must map *that* copy. Not for the memory — it is two pages — but
//! because one of those pages is the clock. When the wall clock is set, or the
//! TSC turns out to be unusable, the kernel writes the new parameters once and
//! every process has to see them. A process holding a private snapshot would
//! keep answering `CLOCK_REALTIME` from a stale offset forever, silently, which
//! is precisely the failure the whole design is arranged to avoid.
//!
//! `fork` is what makes this delicate. `VmMapping::clone_map` gives the child a
//! *copy* of a paged VMO — copy-on-write now, but still a copy — and shares the
//! object only when it is physical, on the grounds that a physical VMO is a
//! window onto a fixed range that both processes must see identically. That
//! description fits the vDSO exactly, so it is built as one: frames allocated
//! once at first use and never freed, wrapped in `VmObject::new_physical`. The
//! sharing then falls out of machinery that already exists and already means
//! what we need it to mean, rather than a special case bolted onto fork.
//!
//! (Physical VMOs default to an uncached policy, which is right for a device
//! BAR and very wrong for code on the hot path of every clock read, so the
//! policy is set back to cached before anything maps it.)
//!
//! ## Where it is mapped
//!
//! Immediately below the process stack, with a page of gap. Not
//! `map(None, ...)`: kernel-chosen placement is first-fit from the bottom of
//! the address space, which lands the image at the end of the loaded ELF —
//! exactly where `brk` grows, and `brk` maps at a fixed address, so the first
//! heap extension would fail. Below the stack is both out of the way and where
//! Linux puts it.

use alloc::sync::Arc;
use core::sync::atomic::{compiler_fence, Ordering};

use kernel_hal::mem::PhysFrame;
use kernel_hal::{PhysAddr, VirtAddr};
use spin::Once;
use zircon_object::object::KernelObject;
use zircon_object::vm::{pages, roundup_pages, CachePolicy, MMUFlags, VmAddressRegion, VmObject};

use linux_vdso::VdsoData;

/// `AT_SYSINFO_EHDR`: the aux-vector entry a C library reads to find the vDSO.
///
/// Its value is the address of the image's ELF header, which is the mapping
/// base — the image is linked with a single `PT_LOAD` at vaddr 0 covering the
/// header, so the two coincide by construction.
pub const AT_SYSINFO_EHDR: u8 = 33;

/// A gap left between the vDSO and the bottom of the stack, so a stack overflow
/// faults instead of running quietly into executable pages.
const STACK_GUARD: usize = zircon_object::vm::PAGE_SIZE;

struct Vdso {
    /// The image, shared by every process that maps it.
    vmo: Arc<VmObject>,
    /// Kernel virtual address of `_vdso_data` inside the image's frames. The
    /// kernel writes the clock parameters straight through here.
    data: *mut VdsoData,
    /// Mapped length: the image rounded up to whole pages.
    len: usize,
}

// `data` points into frames that are allocated once and never freed, and is
// only ever written through `publish`, which the clock notification path may
// call from any CPU. See `publish` for why those writes need no lock.
unsafe impl Send for Vdso {}
unsafe impl Sync for Vdso {}

static VDSO: Once<Option<Vdso>> = Once::new();

/// The vDSO, built on first use. `None` when this kernel has none — either the
/// build had no C compiler, or physical memory for it could not be obtained.
fn vdso() -> Option<&'static Vdso> {
    VDSO.call_once(build).as_ref()
}

fn build() -> Option<Vdso> {
    if !linux_vdso::AVAILABLE {
        warn!("vdso: esta compilacion no incluye imagen; clock_gettime seguira siendo un syscall");
        return None;
    }

    let len = roundup_pages(linux_vdso::IMAGE_LEN);
    let frames = PhysFrame::new_contiguous(pages(len), 0);
    if frames.is_empty() {
        warn!("vdso: no hay memoria fisica contigua para la imagen");
        return None;
    }
    let paddr: PhysAddr = frames[0].paddr();
    // The image outlives every process and is never reclaimed, so the frames
    // are deliberately leaked rather than tracked. Dropping this `Vec` would
    // free them out from under every mapping in the system.
    core::mem::forget(frames);

    let base: VirtAddr = kernel_hal::mem::phys_to_virt(paddr);
    // SAFETY: `base` is the direct-map address of `pages(len)` frames this
    // function just took exclusive ownership of, and nothing else refers to
    // them yet.
    unsafe {
        let dst = core::slice::from_raw_parts_mut(base as *mut u8, len);
        // Zero first: the image is shorter than the pages it occupies, and the
        // tail — including the rest of the `_vdso_data` page — must not expose
        // whatever the allocator handed us to userspace.
        dst.fill(0);
        dst[..linux_vdso::IMAGE_LEN].copy_from_slice(linux_vdso::IMAGE);
    }

    let vmo = VmObject::new_physical(paddr, pages(len));
    // Physical VMOs default to uncached, which for a device window is correct
    // and here would put every instruction of the read path and the clock
    // parameters themselves on uncached memory. Must happen before the first
    // mapping: `set_cache_policy` refuses once a VMO has mappings.
    if let Err(e) = vmo.set_cache_policy(CachePolicy::Cached) {
        warn!("vdso: no se pudo marcar la imagen como cacheable: {:?}", e);
        return None;
    }
    vmo.set_name("vdso");

    let vdso = Vdso {
        vmo,
        data: (base + linux_vdso::DATA_OFFSET) as *mut VdsoData,
        len,
    };

    info!(
        "vdso: imagen en {:#x} ({} paginas), _vdso_data en {:#x}",
        paddr,
        pages(len),
        base + linux_vdso::DATA_OFFSET
    );
    Some(vdso)
}

/// Write the current clock parameters into the image.
///
/// Called on every change to the wall-clock offset or to the TSC's usability,
/// through the observer registered in [`init`]. Cheap enough — three stores —
/// that there is no value in trying to skip redundant calls.
///
/// ## Why there is no lock and no seqlock
///
/// Readers are in userspace and cannot take a kernel lock, so a lock here would
/// protect nothing. A seqlock would work, and the real Linux vDSO uses one,
/// because it publishes a whole coherent snapshot: a clock source, its mask,
/// its shift, a base time and a cycle count that only make sense together.
///
/// This publishes two numbers that do not depend on each other. `tsc_mult`
/// converts ticks to nanoseconds; `wall_off_ns` shifts monotonic to realtime. A
/// reader that catches a fresh one alongside a stale one gets an answer correct
/// for the instant it read, which is everything a clock owes its caller. Both
/// are naturally aligned 64-bit fields, and an aligned 64-bit load on x86_64
/// cannot tear, so each is individually whole. What is left is ordering, and
/// only in one direction: `enabled` must not turn on before the values it
/// vouches for are in memory, and must turn off before they stop being true.
fn publish() {
    let Some(vdso) = vdso() else { return };
    let mult = kernel_hal::timer::vdso_tsc_mult();
    let wall_off_ns = kernel_hal::timer::wall_clock_offset_ns();

    // SAFETY: `data` addresses `_vdso_data` inside frames owned for the
    // lifetime of the system, whose extent the build script has checked lies
    // wholly within the image. Volatile writes because the reader is userspace
    // and invisible to the compiler.
    unsafe {
        let d = vdso.data;
        let enabled = core::ptr::addr_of_mut!((*d).enabled);
        let tsc_mult = core::ptr::addr_of_mut!((*d).tsc_mult);
        let wall = core::ptr::addr_of_mut!((*d).wall_off_ns);

        match mult {
            None => {
                // Turning off: `enabled` goes first, so no reader can act on
                // parameters already known to be wrong.
                core::ptr::write_volatile(enabled, 0);
                compiler_fence(Ordering::SeqCst);
                core::ptr::write_volatile(tsc_mult, 0);
                core::ptr::write_volatile(wall, wall_off_ns);
            }
            Some(mult) => {
                // Turning on, or updating: the parameters land first, so a
                // reader that sees `enabled` sees them too.
                core::ptr::write_volatile(tsc_mult, mult);
                core::ptr::write_volatile(wall, wall_off_ns);
                compiler_fence(Ordering::SeqCst);
                core::ptr::write_volatile(enabled, 1);
            }
        }
    }
}

/// Build the vDSO and start keeping its clock parameters current.
///
/// Idempotent, and safe to call before or after the clock has been calibrated:
/// registering the observer publishes once immediately, and every later change
/// republishes.
pub fn init() {
    // Build first, register second, and never the other way round. The TSC
    // watchdog notifies from the timer interrupt, so the observer can fire in
    // IRQ context on any CPU — and `publish` starts by asking for the image. If
    // an observer could be installed before `VDSO` was initialized, that first
    // notification would run `build` from an interrupt, allocating physical
    // frames underneath whatever the interrupted code was doing. With this
    // order the `call_once` inside `publish` is always already resolved, so it
    // is a plain load and `publish` is nothing but volatile stores.
    if vdso().is_none() {
        return;
    }
    static REGISTERED: Once<()> = Once::new();
    REGISTERED.call_once(|| kernel_hal::timer::set_clock_observer(publish));
}

/// One line describing why userspace is, or is not, reading the clock without
/// a syscall. Reported by `/proc/perf/kernel`.
///
/// Every way this feature can fail to engage is silent — the guest keeps
/// working and clock reads just stay expensive — so without this the only
/// symptom of a broken vDSO is a benchmark number that did not move, which is
/// indistinguishable from the feature not helping.
pub fn status() -> alloc::string::String {
    use alloc::format;
    if !linux_vdso::AVAILABLE {
        return "sin imagen (la compilacion no encontro un cc utilizable)".into();
    }
    let Some(vdso) = vdso() else {
        return "imagen presente pero no instalada (sin memoria fisica)".into();
    };
    // Read back what userspace would read, rather than recomputing it: the
    // question this answers is what is actually published, not what should be.
    // SAFETY: see `publish`.
    let (enabled, mult) = unsafe {
        (
            core::ptr::read_volatile(core::ptr::addr_of!((*vdso.data).enabled)),
            core::ptr::read_volatile(core::ptr::addr_of!((*vdso.data).tsc_mult)),
        )
    };
    if enabled == 0 {
        return "instalada pero inactiva (CPUID no declara el TSC invariante, \
                asi que clock_gettime sigue siendo un syscall; bajo QEMU/TCG \
                eso es inevitable y se fuerza con VDSOFORCE=1)"
            .into();
    }
    format!(
        "activa, tsc_mult={} ({}.{:03} ns/tick)",
        mult,
        mult >> 32,
        ((mult & 0xffff_ffff) * 1000) >> 32,
    )
}

/// Map the vDSO into a process, just below `stack_bottom`, and return the
/// address to publish as `AT_SYSINFO_EHDR`.
///
/// `None` when there is no vDSO to map, or when the mapping fails. Both are
/// non-fatal: without `AT_SYSINFO_EHDR` the C library never looks for a vDSO
/// and every clock read goes to the kernel, exactly as before this existed.
pub fn map_into(vmar: &Arc<VmAddressRegion>, stack_bottom: VirtAddr) -> Option<VirtAddr> {
    init();
    let vdso = vdso()?;

    // Do not advertise a vDSO that cannot answer. It would still be *correct* —
    // every call returns -ENOSYS and the libc falls through to the syscall —
    // but the process would pay an indirect call before every clock read for
    // nothing, which is worse than never having been offered one.
    //
    // Only checked here, at exec. A TSC demoted later leaves already-running
    // processes holding a mapping that has gone quiet; they take the fallback
    // path and keep telling the right time, which is the whole point of the
    // -ENOSYS contract.
    // SAFETY: see `publish`.
    if unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*vdso.data).enabled)) } == 0 {
        return None;
    }

    let top = stack_bottom.checked_sub(STACK_GUARD)?;
    let base = top.checked_sub(vdso.len)?;
    let offset = base.checked_sub(vmar.addr())?;

    // Read and execute, never write. A process that could write the data page
    // could lie to itself about the time and to nothing else, but there is no
    // reason to allow even that — and a writable mapping of a shared physical
    // VMO would let it lie to every other process too.
    let flags = MMUFlags::READ | MMUFlags::EXECUTE | MMUFlags::USER;
    // `map_range: true` — populate both PTEs now rather than taking two page
    // faults on the first clock read. The frames are already there; this is a
    // page-table write, and it makes the very first `clock_gettime` as cheap as
    // every later one instead of paying for the trap this exists to remove.
    match vmar.map_ext(
        Some(offset),
        vdso.vmo.clone(),
        0,
        vdso.len,
        flags,
        flags,
        false,
        true,
    ) {
        Ok(addr) => {
            debug!("vdso: mapeada en {:#x}..{:#x}", addr, addr + vdso.len);
            Some(addr)
        }
        Err(e) => {
            warn!("vdso: no se pudo mapear en {:#x}: {:?}", base, e);
            None
        }
    }
}

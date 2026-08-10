//! Liveness gate for `dyn` fat pointers reached from interrupt context.
//!
//! Every `Box<dyn Fn…>` / `Arc<dyn Fn…>` this crate calls from an IRQ (device
//! handlers, timer callbacks, event-listener wakers, deferred jobs) is two
//! words wide: `{ data, vtable }`. A heap smash — a coroutine stack overflow
//! spilling into neighbouring allocations, or a use-after-free of a one-shot
//! waker — leaves small garbage in one or both words. *Calling* such a pointer
//! is a jump through a null/garbage vtable slot, i.e. a null-range EXECUTE #PF
//! taken in IRQ context with no current thread, which is exactly the
//! unrecoverable cascade `docs/README-crash-repro.md` documents. **Dropping**
//! it is no better: `Drop` for a trait object also dispatches through the same
//! vtable.
//!
//! So the callers gate on [`dyn_fat_ptr_live`] and, when it says the pointer is
//! dead, `core::mem::forget` the value instead of calling or dropping it: leak
//! one closure, keep the machine.
//!
//! The check only ever rejects a pointer that **cannot** be live: a null word,
//! a vtable outside the kernel image, a misaligned vtable, or a vtable that
//! points into the kernel heap (see [`set_vtable_max`]). A genuinely live
//! kernel `dyn` pointer always passes, so a false positive cannot silently
//! disable a working handler. It is a corruption tripwire, not a validator:
//! garbage that happens to look like an aligned `.rodata` address is not caught
//! here.
//!
//! # The heap-range test
//!
//! Real vtables are emitted into `.rodata`, which every target links *below*
//! the `static mut HEAP` in `.bss`. So a vtable word at or above the heap base
//! cannot be genuine — it is a heap pointer that a use-after-free wrote over a
//! live `dyn` object's vtable slot. This is the exact signature observed when a
//! coalesced timer callback's `Box<dyn FnOnce>` had its vtable overwritten with
//! a freed BTree node during VMO teardown: dispatching through it jumps to a
//! data address and faults in IRQ context. Registering the heap base via
//! [`set_vtable_max`] turns that corruption into a caught-and-leaked closure
//! instead of a dead machine.
//!
//! # Why only the vtable gets an address test
//!
//! The `data` word is checked for null and nothing else, on purpose. A
//! *zero-sized* pointee — and a closure that captures nothing is zero-sized —
//! has no allocation, so `Box`/`Arc` store a **dangling** pointer for it:
//! `align_of::<T>()`, i.e. a value as small as 1. `linux_object`'s coalesced
//! DRM timer is exactly that shape
//! (`Box::new(move |_| deliver_pending_drm_timer())`), and rejecting it would
//! be catastrophic rather than merely wrong: the first refusal latches
//! [`heap_smash_suspected`], after which the IRQ and timer paths stop calling
//! *every* closure — keyboard, serial, xHCI and all timers go dead at once, and
//! the machine freezes with no fault to point at. Only the vtable can be
//! meaningfully bounded, because it always points into the kernel image's
//! `.rodata`.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Per-CPU sticky "the heap is no longer trustworthy" flags. A smash on one
/// core must not disable dyn dispatch on every other core.
const MAX_CPU: usize = 64;
static HEAP_SMASH_SUSPECTED: [AtomicBool; MAX_CPU] = [const { AtomicBool::new(false) }; MAX_CPU];

/// Record that kernel memory corruption is suspected on **this** CPU.
/// Idempotent and lock-free: callable from any context, including a hard IRQ.
pub fn note_heap_smash_suspected() {
    let cpu = lock::current_cpu_id() as usize;
    if cpu < MAX_CPU {
        HEAP_SMASH_SUSPECTED[cpu].store(true, Ordering::SeqCst);
    }
}

/// Whether a heap smash has been observed on **this** CPU since boot.
pub fn heap_smash_suspected() -> bool {
    let cpu = lock::current_cpu_id() as usize;
    cpu < MAX_CPU && HEAP_SMASH_SUSPECTED[cpu].load(Ordering::Relaxed)
}

/// First address of the kernel heap, or `0` until the boot code registers it.
///
/// The kernel image links `.rodata` (where every real vtable lives) below
/// `.bss` (where the `static mut HEAP` sits), so **no genuine vtable is ever at
/// or above the heap base**. A fat pointer whose vtable word lands inside the
/// heap is therefore corruption by construction — most often a
/// use-after-free that has had a live `dyn` object's vtable slot overwritten
/// with a heap pointer (a freed BTree node, another allocation's payload, …).
///
/// Zero means "not registered" — hosted/libos builds and early boot leave the
/// heap-range test disabled and fall back to the address-half test alone.
static VTABLE_MAX: AtomicUsize = AtomicUsize::new(0);

/// Register the kernel heap base so [`dyn_fat_ptr_live`] can reject vtables that
/// point into the heap. Call once, from bare-metal heap init, with the heap's
/// lowest address. Idempotent; a later call simply overwrites the bound.
pub fn set_vtable_max(heap_start: usize) {
    VTABLE_MAX.store(heap_start, Ordering::SeqCst);
}

/// Whether `vtable` falls in the heap (or above it), where no real vtable can
/// live. Returns `false` when the bound is unregistered so the test is inert.
#[inline]
fn vtable_in_heap(vtable: usize) -> bool {
    let max = VTABLE_MAX.load(Ordering::Relaxed);
    max != 0 && vtable >= max
}

/// Whether `addr` can be the address of a vtable in the kernel image.
///
/// Every bare-metal target links the kernel into the upper half (x86_64
/// `0xffff_8000_…`, riscv64 `0xffff_ffc0_…`, aarch64 TTBR1 `0xffff_…`), so a
/// sign-extended-negative value is the portable test, and a small or user-half
/// value in a vtable slot is corruption by construction.
///
/// On a hosted build (libos) the whole "kernel" is an ordinary userspace
/// process, so its vtables live at *low* addresses and this test would reject
/// every handler in the system. There is no kernel/user split to check there —
/// the test degrades to "not null", which is all that is knowable.
#[inline]
fn plausible_vtable(addr: usize) -> bool {
    if cfg!(target_os = "none") {
        (addr as isize) < 0
    } else {
        true
    }
}

/// Whether the `dyn` fat pointer stored in `p` still looks callable.
///
/// `P` must be a two-word pointer to an unsized value — `Box<dyn …>`,
/// `Arc<dyn …>`, `&dyn …`. A `P` of any other width returns `true`
/// (nothing to check), so a caller that passes a thin pointer degrades to the
/// old unguarded behaviour rather than rejecting everything.
///
/// Returns `false` — and latches [`heap_smash_suspected`] — when the pointer
/// cannot possibly be live. The caller must then `core::mem::forget` the value:
/// see the module docs for why dropping it is not an option.
pub fn dyn_fat_ptr_live<P>(p: &P) -> bool {
    const WORD: usize = core::mem::size_of::<usize>();
    if core::mem::size_of::<P>() != 2 * WORD {
        return true;
    }
    // SAFETY: `p` is a live `&P` of exactly two words, so both reads are in
    // bounds and naturally aligned (`P` contains pointers, hence is word
    // aligned). Volatile because the words being validated are precisely the
    // ones a concurrent smash may have rewritten — the compiler must not fold
    // these loads into an assumption about a well-formed fat pointer.
    let (data, vtable) = unsafe {
        let words = p as *const P as *const usize;
        (
            core::ptr::read_volatile(words),
            core::ptr::read_volatile(words.add(1)),
        )
    };
    // The vtable is the word that gets *dispatched through*, so it carries the
    // whole test: non-null, inside the kernel image, and word-aligned (a vtable
    // is an array of pointers and is never misaligned). `data` is only checked
    // for null — see the module docs: a zero-sized pointee legitimately stores
    // a tiny dangling pointer there, and rejecting it freezes the machine.
    let in_heap = vtable_in_heap(vtable);
    let live =
        data != 0 && vtable != 0 && plausible_vtable(vtable) && vtable % WORD == 0 && !in_heap;
    if !live {
        // Latching decision. A vtable that points INTO the heap is the
        // signature of a *localized* use-after-free — one freed object's slot
        // now holds a heap pointer (e.g. a recycled BTree node landing on a
        // one-shot timer callback). Skipping and leaking just that closure lets
        // the rest of the machine keep running, so we do NOT set the sticky
        // smash flag for it: latching would stop *all* IRQ and timer dyn
        // dispatch (keyboard, serial, xHCI, every timer) and freeze a machine
        // that only lost one callback. Every other dead shape — null,
        // misaligned, user-half — is ambiguous corruption that may be spreading,
        // so stay conservative and latch as before.
        if !in_heap {
            note_heap_smash_suspected();
        }
        warn!(
            "[fat-ptr] dead dyn pointer: data={:#x} vtable={:#x}{} — \
             skipping and leaking it instead of dispatching through the vtable",
            data,
            vtable,
            if in_heap {
                " (vtable points into the heap: use-after-free; contained, machine keeps running)"
            } else {
                ""
            },
        );
    }
    live
}

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
//! a vtable in the user half of the address space, or a misaligned vtable. A
//! genuinely live kernel `dyn` pointer always passes, so a false positive
//! cannot silently disable a working handler. It is a corruption tripwire, not
//! a validator: garbage that happens to look like an aligned kernel address is
//! not caught here.

use core::sync::atomic::{AtomicBool, Ordering};

/// Sticky "the heap is no longer trustworthy" flag.
///
/// Set by the first dead fat pointer seen here, and by callers that spot the
/// same smash signature in their own state (see `kernel_hal::timer`'s clock
/// observer). Once set, IRQ paths stop calling *any* registered closure rather
/// than gambling on each one individually: after a smash the next dead pointer
/// is a matter of time, and one that lands mid-`Drop` cannot be undone.
///
/// Deliberately one-way. Nothing clears it — there is no evidence that would
/// justify declaring a smashed heap healthy again.
static HEAP_SMASH_SUSPECTED: AtomicBool = AtomicBool::new(false);

/// Record that kernel memory corruption is suspected (see
/// [`HEAP_SMASH_SUSPECTED`]). Idempotent and lock-free: callable from any
/// context, including a hard IRQ or a panic path.
pub fn note_heap_smash_suspected() {
    HEAP_SMASH_SUSPECTED.store(true, Ordering::SeqCst);
}

/// Whether a heap smash has been observed since boot.
pub fn heap_smash_suspected() -> bool {
    HEAP_SMASH_SUSPECTED.load(Ordering::Relaxed)
}

/// Whether `addr` can be a kernel-half virtual address.
///
/// Every supported target puts kernel mappings in the upper half (x86_64
/// `0xffff_8000_…`, riscv64 `0xffff_ffc0_…`, aarch64 TTBR1 `0xffff_…`), so a
/// sign-extended-negative value is the portable test. A user-half or small
/// value in a vtable slot is corruption by construction: a vtable lives in the
/// kernel image's `.rodata`.
#[inline]
fn kernel_half(addr: usize) -> bool {
    (addr as isize) < 0
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
    // strictest test: non-null, kernel-half, and word-aligned (a vtable is an
    // array of pointers and is never misaligned). `data` only has to be
    // non-null and kernel-half — a `Box`/`Arc` payload is a kernel heap
    // allocation, but its alignment is the pointee's, not the word's.
    let live =
        data != 0 && kernel_half(data) && vtable != 0 && kernel_half(vtable) && vtable % WORD == 0;
    if !live {
        note_heap_smash_suspected();
        warn!(
            "[fat-ptr] dead dyn pointer: data={:#x} vtable={:#x} — \
             skipping and leaking it instead of dispatching through the vtable",
            data, vtable
        );
    }
    live
}

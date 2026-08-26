//! Compile-time cross-subsystem invariants.
//!
//! These `const` assertions enforce relationships between constants that live
//! in different crates and subsystems. They cost nothing at runtime: a violated
//! invariant fails the build with a clear message instead of producing a subtle
//! miscompile or a panic at boot.
//!
//! The pattern is borrowed from the legacy Eclipse kernel's `invariants.rs`,
//! adapted to zCore's crate layout. Keep an assertion here whenever two
//! constants in different places *must* agree but the compiler would not
//! otherwise notice them drifting apart.

use zircon_object::vm::{PAGE_SIZE, PAGE_SIZE_LOG2, USER_STACK_PAGES};

// --- Page geometry -----------------------------------------------------------

/// Page size must be a power of two: mask arithmetic throughout the VM layer
/// (e.g. `addr & (PAGE_SIZE - 1)` to isolate the offset) is only correct when
/// `PAGE_SIZE` is `2^n`.
const _: () = assert!(PAGE_SIZE.is_power_of_two());

/// `PAGE_SIZE` and `PAGE_SIZE_LOG2` are used interchangeably (shift vs.
/// multiply) and must never disagree.
const _: () = assert!(1usize << PAGE_SIZE_LOG2 == PAGE_SIZE);

// --- User stack --------------------------------------------------------------

/// New threads are given a `USER_STACK_PAGES`-page stack; a zero here would
/// hand them an empty stack and fault on the first push.
const _: () = assert!(USER_STACK_PAGES > 0);

/// The initial user stack should stay within a sane bound (16 MiB). This is a
/// tripwire against an accidental fat-finger (e.g. an extra zero) rather than a
/// hard architectural limit.
const _: () = assert!(USER_STACK_PAGES * PAGE_SIZE <= 16 * 1024 * 1024);

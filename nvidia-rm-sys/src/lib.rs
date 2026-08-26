//! FFI bridge to vendored NVIDIA open-gpu-kernel-modules C source for
//! Eclipse. `os_interface` implements NVIDIA's real os-interface.h ABI
//! (transcribed verbatim, MIT) against Eclipse's own primitives; `hooks`
//! is the registration point for the handful of operations (PCI config,
//! MMIO mapping, I/O ports, timing) that only `drivers` can provide
//! without creating a dependency cycle. The smoke test below predates
//! both and stays as a standing canary that the C-compile + FFI-link
//! pipeline itself still works -- see build.rs and vendor/smoketest.c.
#![no_std]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

pub mod fnv_hash;
pub mod hooks;
pub mod os_boundary;
pub mod os_interface;
pub mod os_services;
pub mod rm_init;
pub mod survival;
pub mod types;

use core::sync::atomic::{AtomicU32, Ordering};

extern "C" {
    fn nvrm_smoketest_add(a: u32, b: u32) -> u32;
}

/// Set by the C side via `nvrm_smoketest_log` so callers can confirm the
/// C-to-Rust callback direction actually ran (not just C-to-C linkage).
static LAST_LOGGED: AtomicU32 = AtomicU32::new(0);

/// Called from vendor/smoketest.c -- proves C code can call back into a
/// Rust-implemented function, the same shape `os-interface.h` will need.
#[no_mangle]
pub extern "C" fn nvrm_smoketest_log(value: u32) {
    LAST_LOGGED.store(value, Ordering::SeqCst);
}

/// Runs the round trip (Rust -> C -> Rust) and returns `(result, logged)`.
/// Both should equal `a + b` if the pipeline works end to end.
pub fn smoke_test(a: u32, b: u32) -> (u32, u32) {
    let result = unsafe { nvrm_smoketest_add(a, b) };
    (result, LAST_LOGGED.load(Ordering::SeqCst))
}

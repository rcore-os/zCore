//! Hosted (libos) stub of the bare-metal coroutine stack guard.
//!
//! Hosted builds run on the host's threads and page tables — there is no
//! kernel physmap through which a physical-memory write could alias a live
//! executor stack — so the alias probe is trivially false. This keeps callers
//! (e.g. the physical-VMO diagnostics in `zircon-object`) building unchanged
//! on both substrates.

/// See `bare::stack_guard::paddr_aliases_stack`; never true on hosted builds.
pub fn paddr_aliases_stack(_paddr: usize, _len: usize) -> bool {
    false
}

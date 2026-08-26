//! Implementation of the small subset of NVIDIA's internal "OBJOS" service
//! surface that real vendored RM files (core/thread_state.c, the nvport
//! libraries, diagnostics/nvlog_printf.c) call directly -- distinct from
//! the `os-interface.h` ABI in `os_interface.rs`. Real signatures
//! transcribed from src/nvidia/generated/g_os_nvoc.h (MIT,
//! NVIDIA/open-gpu-kernel-modules); NVIDIA's own real implementation of
//! these lives in the Linux-specific arch/nvalloc/unix/src tree, which
//! Eclipse has no equivalent of, so each is backed by Eclipse's own
//! primitives (via `crate::hooks`) or a safe default, same convention as
//! `os_interface.rs`.
//!
//! A few functions that originally looked missing from this same header
//! (osGetMaximumCoreCount, osReadRegistryDword/String, g_pSys,
//! gpumgrGetCurrentGpuInstance, threadPriorityStateAlloc/Free,
//! rcdbAddAssertJournalRecWithLine) turned out to already have real
//! implementations in other vendored files once the full RM core linked
//! (os_init.c, system.c, gpu_mgr.c, locks_common.c, journal.c) --
//! confirmed by an actual `duplicate symbol` link error against a hand-
//! written stand-in, not assumed -- so they are deliberately NOT
//! duplicated here.
#![allow(non_snake_case)]

use crate::hooks::with_hooks;
use crate::types::*;

#[no_mangle]
pub extern "C" fn osGetCurrentThread(handle: *mut NvU64) -> NV_STATUS {
    if handle.is_null() {
        return NV_ERR_INVALID_ARGUMENT;
    }
    unsafe { *handle = 0 };
    NV_OK
}

#[no_mangle]
pub extern "C" fn osGetCurrentProcessorNumber() -> NvU32 {
    0
}

#[no_mangle]
pub extern "C" fn osGetCurrentProcessFlags() -> NvU32 {
    0
}

#[no_mangle]
pub extern "C" fn osDelayUs(microseconds: NvU32) -> NV_STATUS {
    with_hooks((), |h| h.delay_us(microseconds));
    NV_OK
}

#[no_mangle]
pub extern "C" fn osGetMonotonicTimeNs() -> NvU64 {
    with_hooks(0, |h| h.monotonic_time_ns())
}

// GPU_TIMEOUT_FLAGS_OSTIMER = NVBIT(3) (gpu_timeout.h). This MUST be set:
// the RM's timeout engine (_checkTimeout, gpu_timeout.c) starts every check
// at status = NV_OK and only ever returns NV_ERR_TIMEOUT from inside a branch
// gated on one of the timer-source flags (OSTIMER / OSDELAY / TMR). With flags
// = 0 the timeout is structurally disabled -- _checkTimeout returns NV_OK
// forever, so any gpuTimeoutCondWait whose condition never comes true (e.g.
// kgspExecuteSequencerCommand_TU102 polling BSI_SECURE_SCRATCH_14 for the SEC2
// GSP-RM resume handoff) spins the CPU with interrupts off and hard-hangs the
// whole box instead of timing out. The real Linux osGetTimeoutParams
// (arch/nvalloc/unix/src/os.c) returns GPU_TIMEOUT_FLAGS_OSTIMER, which routes
// the check to osGetCurrentTick (which Eclipse now backs with a real TSC
// clock), so this matches it.
const GPU_TIMEOUT_FLAGS_OSTIMER: NvU32 = 1 << 3;

#[no_mangle]
pub extern "C" fn osGetTimeoutParams(
    _gpu: *mut c_void,
    time_out_us: *mut NvU32,
    scale: *mut NvU32,
    flags: *mut NvU32,
) {
    unsafe {
        if !time_out_us.is_null() {
            // Real Linux graphics-mode default (os.c: 4 * 1000000). Long enough
            // not to trip on a healthy multi-second GSP bootstrap, short enough
            // that a genuinely stuck poll reports a clean NV_ERR_TIMEOUT.
            *time_out_us = 4_000_000;
        }
        if !scale.is_null() {
            *scale = 1;
        }
        if !flags.is_null() {
            *flags = GPU_TIMEOUT_FLAGS_OSTIMER;
        }
    }
}

#[no_mangle]
pub extern "C" fn osSchedule() -> NV_STATUS {
    NV_OK
}

#[no_mangle]
pub extern "C" fn osGetSystemTime(sec: *mut NvU32, usec: *mut NvU32) -> NV_STATUS {
    let ns = with_hooks(0u64, |h| h.monotonic_time_ns());
    unsafe {
        if !sec.is_null() {
            *sec = (ns / 1_000_000_000) as NvU32;
        }
        if !usec.is_null() {
            *usec = ((ns / 1_000) % 1_000_000) as NvU32;
        }
    }
    NV_OK
}

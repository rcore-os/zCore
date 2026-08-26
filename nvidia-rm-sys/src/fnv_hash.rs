//! FFI bindings for the first real (non-hand-written) vendored NVIDIA
//! source: src/nvidia/src/libraries/fnv_hash/fnv_hash.c. Only linkable
//! once the submodule is checked out and build.rs actually compiles it
//! (see build.rs's build_first_real_nvidia_file) -- calling this without
//! the submodule present fails at kernel link time with an unresolved
//! symbol, same as any other hard dependency on vendored code.
use crate::types::{NvU32, NvU64, NvU8};

extern "C" {
    fn fnv1Hash64(data: *const NvU8, data_len: NvU32) -> NvU64;
}

/// Safe wrapper around NVIDIA's real `fnv1Hash64`. Used as the first
/// real-hardware proof that actual vendored NVIDIA C (not the hand-written
/// smoke test) runs correctly inside Eclipse's kernel.
pub fn fnv1_hash64(data: &[u8]) -> u64 {
    unsafe { fnv1Hash64(data.as_ptr(), data.len() as NvU32) }
}

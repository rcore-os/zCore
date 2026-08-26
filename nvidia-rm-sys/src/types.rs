//! C ABI types matching NVIDIA's real `nvtypes.h` / `nvstatus.h` exactly.
//! Values below are transcribed from the real headers (not guessed) --
//! getting these wrong would silently break RM's own error-handling logic
//! once real RM source is linked in.
#![allow(non_camel_case_types)]

pub type NvU8 = u8;
pub type NvS8 = i8;
pub type NvU16 = u16;
pub type NvS16 = i16;
pub type NvU32 = u32;
pub type NvS32 = i32;
pub type NvU64 = u64;
pub type NvS64 = i64;
/// NVIDIA represents NvBool as `unsigned char` (0/1), not a real C `bool`.
pub type NvBool = u8;
pub const NV_TRUE: NvBool = 1;
pub const NV_FALSE: NvBool = 0;

pub type NV_STATUS = NvU32;

// Transcribed from src/common/sdk/nvidia/inc/nvstatuscodes.h -- exact
// values, not placeholders.
pub const NV_OK: NV_STATUS = 0x00000000;
pub const NV_ERR_GENERIC: NV_STATUS = 0x0000FFFF;
pub const NV_ERR_BUSY_RETRY: NV_STATUS = 0x00000003;
pub const NV_ERR_INVALID_ARGUMENT: NV_STATUS = 0x0000001F;
pub const NV_ERR_INSUFFICIENT_RESOURCES: NV_STATUS = 0x0000001A;
pub const NV_ERR_INVALID_POINTER: NV_STATUS = 0x0000003D;
pub const NV_ERR_INVALID_STATE: NV_STATUS = 0x00000040;
pub const NV_ERR_NO_MEMORY: NV_STATUS = 0x00000051;
pub const NV_ERR_NOT_READY: NV_STATUS = 0x00000055;
pub const NV_ERR_NOT_SUPPORTED: NV_STATUS = 0x00000056;
pub const NV_ERR_OPERATING_SYSTEM: NV_STATUS = 0x00000059;
pub const NV_ERR_TIMEOUT: NV_STATUS = 0x00000065;

pub type c_void = core::ffi::c_void;
pub type c_char = core::ffi::c_char;

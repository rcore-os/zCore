//! Defensive ceilings against a buggy or hostile compositor.
//!
//! Every value a Wayland server hands this client (output count, configure
//! sizes, HiDPI scales) flows into allocation or render-loop math. These caps
//! bound the damage of an absurd value: the wallpaper skips the offending
//! output instead of allocating gigabytes, wrapping size arithmetic, or
//! pinning a core rendering a preposterous buffer.
//!
//! (Reconstructed: the original module was referenced by `mod fill_guard;`
//! but its file was never committed, leaving the crate unbuildable.)

/// Most `wl_output` globals tracked. A server announcing more is either
/// broken or adversarial; extra outputs are ignored with a log line.
pub const MAX_OUTPUTS: usize = 16;

/// Most simultaneous background surfaces (at most one per output).
pub const MAX_BACKGROUNDS: usize = 16;

/// Largest buffer side, in pixels, after applying the HiDPI scale. Covers a
/// 16K panel; anything past it is skipped (and the scale is walked back down
/// before giving up — see `build_frames`).
pub const MAX_BUFFER_DIM: u32 = 16384;

/// Largest buffer area this software renderer will paint (64 Mpx — an 8K
/// frame is ~33 Mpx). Past this the per-frame CPU cost stops being a
/// wallpaper and starts being a space heater; the output is skipped.
pub const MAX_BUFFER_PIXELS: usize = 64 * 1024 * 1024;

/// Most retired shm mappings held while waiting for the compositor to
/// Release their buffers. Holding is deliberate — unmapping early would be a
/// use-after-free on the compositor's side of the pool — so past this cap we
/// only log; each entry is at most one pool (~16 MiB at 1080p).
pub const MAX_RETIRED_MAPS: usize = 8;

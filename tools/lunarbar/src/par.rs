//! Render-pass helpers for the full-surface pixel blits.
//!
//! Originally used `std::thread::scope` to split row-bands across CPU cores.
//! Eclipse OS's kernel does not reliably support the cross-thread
//! stack-reference captures that `std::thread::scope` relies on: spawned
//! threads faulted with a null-range kernel page fault at offset 0x18 (the
//! captured source-buffer slice pointer inside the closure struct). All passes
//! now run serially, which is functionally identical and fast enough for every
//! buffer lunarbar produces (bars are small; even a full-output overlay
//! swizzle completes in well under 5 ms serially).

/// Split `data` — a row-major image buffer of `h` rows, each `row_stride`
/// elements — into horizontal bands and run `f(y0, band)` on each.
/// `y0` is the band's first row index.
///
/// Currently runs as a single serial call. The signature is kept multi-band
/// compatible so callers need not change if parallelism is re-enabled later.
pub fn par_rows<T, F>(data: &mut [T], h: usize, row_stride: usize, f: F)
where
    T: Send,
    F: Fn(usize, &mut [T]) + Sync,
{
    let _ = (h, row_stride); // parameters reserved for future multi-band use
    f(0, data);
}

//! Dependency-free data parallelism for the full-surface render passes.
//!
//! The wallpaper's static base is per-pixel float math over the whole output
//! (`scene::render_base`): a vertical gradient and a dithered quantise, each
//! O(width*height). On a 1080p+ output that is millions of pixels rendered on
//! one core before the first frame can be shown. Splitting those passes across
//! the machine's CPUs cuts that first-paint latency by roughly the core count.
//!
//! No crate is added for this: `std::thread::scope` (stable since 1.63) lets a
//! borrowed buffer be split into disjoint, non-overlapping row-bands and handed
//! one per thread, so each band is written with no locking and the result is
//! byte-identical to the serial version.

/// Worker-thread count for a full-surface pass: the machine's parallelism, or
/// 1 when it can't be determined (a single-CPU guest).
fn worker_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Split `data` — a row-major image buffer of `h` rows, each `row_stride`
/// elements — into up to `worker_threads()` disjoint horizontal bands and run
/// `f(y0, band)` on each concurrently, where `y0` is the band's first row index.
///
/// Bands never overlap, so `f` may write its band freely with no
/// synchronisation, and reading shared immutable data (the source buffer, the
/// layout) by absolute index is safe. Falls back to one serial call on a single
/// CPU or when the buffer is too small for thread-spawn cost to pay off.
pub fn par_rows<T, F>(data: &mut [T], h: usize, row_stride: usize, f: F)
where
    T: Send,
    F: Fn(usize, &mut [T]) + Sync,
{
    let n = worker_threads().min(h.max(1));
    // Below ~256 KiB the spawn/join cost dominates the work; stay serial.
    if n <= 1 || data.len() < 256 * 1024 || row_stride == 0 {
        f(0, data);
        return;
    }
    let band_rows = h.div_ceil(n);
    let band_len = band_rows * row_stride;
    std::thread::scope(|s| {
        let mut y0 = 0usize;
        for band in data.chunks_mut(band_len) {
            let yy = y0;
            let fref = &f;
            s.spawn(move || fref(yy, band));
            y0 += band_rows;
        }
    });
}

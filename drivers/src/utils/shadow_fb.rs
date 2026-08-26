//! A double-buffered ("shadow") framebuffer for the graphic console.
//!
//! All console drawing happens into a CPU-side ARGB8888 buffer kept in normal,
//! cached RAM, which is cheap to both read and write. Dirty regions are then
//! pushed to the real display / GPU framebuffer in bulk via
//! [`DisplayScheme::blit_from`] followed by a single
//! [`DisplayScheme::flush`].
//!
//! This avoids the two patterns that make a naive framebuffer console crawl on
//! real hardware:
//!  * per-pixel MMIO writes through the PCI BAR aperture, and
//!  * reading back VRAM during console scrolling (uncached/write-combining GPU
//!    memory is extremely slow to read).
//!
//! The same abstraction serves both backends equally: an NVIDIA GPU receives
//! the bulk blit straight into its BAR-mapped VRAM, while a virtio-gpu device
//! receives it into its host-shared framebuffer and the trailing `flush`
//! triggers the host transfer.

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use lock::Mutex;

use crate::scheme::DisplayScheme;

/// Inclusive-exclusive dirty bounding box in pixels: `[x0, y0, x1, y1)`.
type DirtyRect = (usize, usize, usize, usize);

struct ShadowInner {
    /// ARGB8888 pixels, row-major, `width` pixels per row.
    data: Vec<u32>,
    /// Smallest rectangle covering everything changed since the last present,
    /// or `None` when the shadow and the real framebuffer are in sync.
    dirty: Option<DirtyRect>,
    /// Pixel rectangle `(x, y, w, h)` where the inverted text cursor was last
    /// drawn directly to the device, so it can be erased on the next present.
    prev_cursor: Option<DirtyRect>,
}

/// A CPU-side shadow of the display framebuffer with dirty-region tracking.
///
/// Interior mutability lets the glyph renderer (which only has shared access
/// through the `DrawTarget`) and the console scroll/fill paths share one
/// buffer. Concurrency is not a concern in practice — the whole graphic console
/// is already serialized behind a single lock — but the internal [`Mutex`]
/// keeps the type `Send + Sync` and the accesses sound.
pub struct ShadowFramebuffer {
    width: usize,
    height: usize,
    inner: Mutex<ShadowInner>,
}

impl ShadowFramebuffer {
    /// Create a black shadow buffer of `width` x `height` pixels.
    pub fn new(width: usize, height: usize) -> Arc<Self> {
        Arc::new(Self {
            width,
            height,
            inner: Mutex::new(ShadowInner {
                data: vec![0; width.saturating_mul(height)],
                dirty: None,
                prev_cursor: None,
            }),
        })
    }

    /// Width in pixels.
    #[inline]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    #[inline]
    pub fn height(&self) -> usize {
        self.height
    }

    #[inline]
    fn mark(inner: &mut ShadowInner, x0: usize, y0: usize, x1: usize, y1: usize) {
        inner.dirty = Some(match inner.dirty {
            Some((ax0, ay0, ax1, ay1)) => (ax0.min(x0), ay0.min(y0), ax1.max(x1), ay1.max(y1)),
            None => (x0, y0, x1, y1),
        });
    }

    /// Write a batch of `(x, y, argb)` pixels (used by the glyph renderer).
    ///
    /// Taking an iterator lets a whole glyph be rendered under a single lock.
    pub fn put_pixels(&self, pixels: impl Iterator<Item = (usize, usize, u32)>) {
        let (w, h) = (self.width, self.height);
        let mut g = self.inner.lock();
        for (x, y, argb) in pixels {
            if x >= w || y >= h {
                continue;
            }
            g.data[y * w + x] = argb;
            Self::mark(&mut g, x, y, x + 1, y + 1);
        }
    }

    /// Fill a rectangle (pixel coordinates) with a single ARGB8888 color.
    pub fn fill_rect(&self, x: usize, y: usize, w: usize, h: usize, argb: u32) {
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        if x >= x1 || y >= y1 {
            return;
        }
        let width = self.width;
        let mut g = self.inner.lock();
        for yy in y..y1 {
            for px in &mut g.data[yy * width + x..yy * width + x1] {
                *px = argb;
            }
        }
        Self::mark(&mut g, x, y, x1, y1);
    }

    /// Copy a rectangle within the shadow buffer (`memmove` semantics), used for
    /// fast console scrolling entirely in cached RAM.
    pub fn copy_rect(&self, sx: usize, sy: usize, dx: usize, dy: usize, w: usize, h: usize) {
        let w = w
            .min(self.width.saturating_sub(sx))
            .min(self.width.saturating_sub(dx));
        let h = h
            .min(self.height.saturating_sub(sy))
            .min(self.height.saturating_sub(dy));
        if w == 0 || h == 0 {
            return;
        }
        let width = self.width;
        let mut g = self.inner.lock();
        if dy <= sy {
            for r in 0..h {
                let s = (sy + r) * width + sx;
                let d = (dy + r) * width + dx;
                g.data.copy_within(s..s + w, d);
            }
        } else {
            for r in (0..h).rev() {
                let s = (sy + r) * width + sx;
                let d = (dy + r) * width + dx;
                g.data.copy_within(s..s + w, d);
            }
        }
        Self::mark(&mut g, dx, dy, dx + w, dy + h);
    }

    /// Clear the whole shadow buffer to `argb` and mark it fully dirty.
    pub fn clear(&self, argb: u32) {
        let mut g = self.inner.lock();
        for px in g.data.iter_mut() {
            *px = argb;
        }
        g.dirty = Some((0, 0, self.width, self.height));
    }

    /// Push the dirty region to the real display and flush it.
    ///
    /// Does nothing when nothing changed since the last present. The dirty
    /// sub-rectangle is blitted in one shot via [`DisplayScheme::blit_from`]
    /// (whose stride argument lets it address a window of the full-width shadow
    /// without copying), and a single [`DisplayScheme::flush`] follows for
    /// devices that need it (virtio-gpu).
    pub fn present(&self, display: &dyn DisplayScheme) {
        let mut g = self.inner.lock();
        let Some((x0, y0, x1, y1)) = g.dirty.take() else {
            return;
        };
        let x0 = x0.min(self.width);
        let y0 = y0.min(self.height);
        let x1 = x1.min(self.width);
        let y1 = y1.min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let start = y0 * self.width + x0;
        display.blit_from(
            x0 as u32,
            y0 as u32,
            &g.data[start..],
            self.width,
            w as u32,
            h as u32,
        );
        drop(g);
        if display.need_flush() {
            let _ = display.flush();
        }
    }

    /// Present the dirty region and overlay a blinking text cursor.
    ///
    /// `cursor` is the cell to highlight `(col, row)` or `None` to hide it;
    /// `cw`/`ch` are the character cell size in pixels. The cursor is drawn as
    /// an inverted block straight to the device (never stored in the shadow, so
    /// it can be erased cleanly) by reading the cell's pixels from the shadow,
    /// XOR-ing them and blitting them back. Because it is always rebuilt from
    /// the clean shadow, redrawing the same cell is idempotent.
    pub fn present_with_cursor(
        &self,
        display: &dyn DisplayScheme,
        cursor: Option<(usize, usize)>,
        cw: usize,
        ch: usize,
    ) {
        let mut g = self.inner.lock();

        // 1. Flush the dirty content region.
        if let Some((x0, y0, x1, y1)) = g.dirty.take() {
            let x0 = x0.min(self.width);
            let y0 = y0.min(self.height);
            let x1 = x1.min(self.width);
            let y1 = y1.min(self.height);
            if x0 < x1 && y0 < y1 {
                let start = y0 * self.width + x0;
                display.blit_from(
                    x0 as u32,
                    y0 as u32,
                    &g.data[start..],
                    self.width,
                    (x1 - x0) as u32,
                    (y1 - y0) as u32,
                );
            }
        }

        // Pixel rectangle of the requested cursor cell, clamped to the screen.
        let new_rect = cursor.and_then(|(cx, cy)| {
            let x = (cx * cw).min(self.width);
            let y = (cy * ch).min(self.height);
            let w = cw.min(self.width - x);
            let h = ch.min(self.height - y);
            if w == 0 || h == 0 {
                None
            } else {
                Some((x, y, w, h))
            }
        });

        // 2. Erase the previously drawn cursor if it has moved or is hidden.
        if let Some(prev) = g.prev_cursor {
            if Some(prev) != new_rect {
                Self::blit_cell(display, &g.data, self.width, prev, false);
            }
        }

        // 3. Draw the cursor (inverted) at its new position.
        if let Some(rect) = new_rect {
            Self::blit_cell(display, &g.data, self.width, rect, true);
        }
        g.prev_cursor = new_rect;

        drop(g);
        if display.need_flush() {
            let _ = display.flush();
        }
    }

    /// Blit one character cell from the shadow to the device, optionally
    /// inverting it (for the cursor). `rect` is `(x, y, w, h)` in pixels.
    fn blit_cell(
        display: &dyn DisplayScheme,
        data: &[u32],
        width: usize,
        rect: DirtyRect,
        invert: bool,
    ) {
        let (x, y, w, h) = rect;
        if invert {
            let mut tmp = Vec::with_capacity(w * h);
            for r in 0..h {
                let base = (y + r) * width + x;
                for c in 0..w {
                    tmp.push(data[base + c] ^ 0x00FF_FFFF);
                }
            }
            display.blit_from(x as u32, y as u32, &tmp, w, w as u32, h as u32);
        } else {
            let start = y * width + x;
            display.blit_from(
                x as u32,
                y as u32,
                &data[start..],
                width,
                w as u32,
                h as u32,
            );
        }
    }
}

use super::Scheme;
use crate::DeviceResult;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor(u32);

/// Color format for one pixel. `RGB888` means R in bits 16-23, G in bits 8-15 and B in bits 0-7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorFormat {
    RGB332,
    RGB565,
    RGB888,
    ARGB8888,
}

#[derive(Debug)]
pub struct Rectangle {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// 2D acceleration capabilities advertised by a display / GPU driver.
///
/// All capabilities default to `false`, meaning the generic software
/// implementations in [`DisplayScheme`] are used. Drivers that can offload
/// these operations to GPU-mapped memory (NVIDIA VRAM over the PCI BAR) or to
/// a host-shared framebuffer (virtio-gpu in QEMU/VirtualBox) override the
/// corresponding methods and set the matching flag, so callers (the graphic
/// console, DRM, ...) can prefer the accelerated 2D path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccelCaps {
    /// Bulk rectangle fill is accelerated.
    pub fill: bool,
    /// Framebuffer-to-framebuffer copy (e.g. console scroll) is accelerated.
    pub copy: bool,
    /// CPU-buffer-to-framebuffer blit (double buffering) is accelerated.
    pub blit: bool,
}

pub struct FrameBuffer<'a> {
    raw: &'a mut [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct DisplayInfo {
    /// visible width
    pub width: u32,
    /// visible height
    pub height: u32,
    /// Number of bytes between each row of the frame buffer.
    pub pitch: u32,
    /// color encoding format of RGBA
    pub format: ColorFormat,
    /// frame buffer base virtual address
    pub fb_base_vaddr: usize,
    /// frame buffer size
    pub fb_size: usize,
}

impl RgbColor {
    #[inline]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    #[inline]
    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }

    #[inline]
    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }

    #[inline]
    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    #[inline]
    pub const fn raw_value(self) -> u32 {
        self.0
    }
}

impl ColorFormat {
    /// Number of bits per pixel.
    #[inline]
    pub const fn depth(self) -> u8 {
        match self {
            Self::RGB332 => 8,
            Self::RGB565 => 16,
            Self::RGB888 => 24,
            Self::ARGB8888 => 32,
        }
    }

    /// Number of bytes per pixel.
    #[inline]
    pub const fn bytes(self) -> u8 {
        self.depth() / 8
    }
}

impl<'a> FrameBuffer<'a> {
    /// # Safety
    ///
    /// This function is unsafe because it created the `FrameBuffer` structure
    /// from the raw pointer.
    pub unsafe fn from_raw_parts_mut(ptr: *mut u8, len: usize) -> Self {
        Self {
            raw: core::slice::from_raw_parts_mut(ptr, len),
        }
    }

    pub fn from_slice(slice: &'a mut [u8]) -> Self {
        Self { raw: slice }
    }

    /// # Safety
    ///
    /// This function is unsafe because the caller must ensure `offset` does
    /// not exceed the frame buffer size.
    pub unsafe fn write_color(&mut self, offset: usize, color: RgbColor, format: ColorFormat) {
        const fn pack_channel(
            r_val: u8,
            _r_bits: u8,
            g_val: u8,
            g_bits: u8,
            b_val: u8,
            b_bits: u8,
        ) -> u32 {
            ((r_val as u32) << (g_bits + b_bits)) | ((g_val as u32) << b_bits) | b_val as u32
        }

        let (r, g, b) = (color.r(), color.g(), color.b());
        let ptr = self.raw.as_mut_ptr().add(offset);
        let dst = core::slice::from_raw_parts_mut(ptr, 4);
        match format {
            ColorFormat::RGB332 => {
                *ptr = pack_channel(r >> (8 - 3), 3, g >> (8 - 3), 3, b >> (8 - 2), 2) as u8
            }
            ColorFormat::RGB565 => {
                *(ptr as *mut u16) =
                    pack_channel(r >> (8 - 5), 5, g >> (8 - 6), 6, b >> (8 - 5), 5) as u16
            }
            ColorFormat::RGB888 => {
                dst[2] = r;
                dst[1] = g;
                dst[0] = b;
            }
            ColorFormat::ARGB8888 => *(ptr as *mut u32) = color.raw_value(),
        }
    }
}

impl<'a> core::ops::Deref for FrameBuffer<'a> {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.raw
    }
}

impl<'a> core::ops::DerefMut for FrameBuffer<'a> {
    #[allow(clippy::needless_borrow)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.raw
    }
}

impl DisplayInfo {
    /// Number of bytes between each row of the frame buffer.
    #[inline]
    pub const fn pitch(self) -> u32 {
        if self.pitch != 0 {
            self.pitch
        } else {
            self.width * self.format.bytes() as u32
        }
    }
}

pub trait DisplayScheme: Scheme {
    fn info(&self) -> DisplayInfo;

    /// Returns the framebuffer.
    fn fb(&self) -> FrameBuffer<'_>;

    /// Report the 2D acceleration capabilities of this device.
    ///
    /// The default is "no acceleration"; the generic software paths below are
    /// then used. Drivers override this together with the relevant methods.
    #[inline]
    fn accel_caps(&self) -> AccelCaps {
        AccelCaps::default()
    }

    /// Write pixel color.
    #[inline]
    fn draw_pixel(&self, x: u32, y: u32, color: RgbColor) {
        let info = self.info();
        if x >= info.width || y >= info.height {
            return;
        }
        let offset =
            (y as usize * info.pitch() as usize) + (x as usize * info.format.bytes() as usize);
        if offset < info.fb_size {
            unsafe { self.fb().write_color(offset, color, info.format) };
        }
    }

    /// Fill a given rectangle with `color`.
    ///
    /// The generic implementation acquires the framebuffer once and writes it
    /// row by row, instead of going through [`draw_pixel`](Self::draw_pixel) per
    /// pixel (which re-derives the framebuffer slice and re-checks bounds on
    /// every pixel). For the common ARGB8888 format this becomes a tight
    /// word-store loop, which on GPU-mapped (write-combining) memory is several
    /// times faster than the per-pixel path.
    fn fill_rect(&self, rect: &Rectangle, color: RgbColor) {
        let info = self.info();
        let left = rect.x.min(info.width);
        let right = rect.x.saturating_add(rect.width).min(info.width);
        let top = rect.y.min(info.height);
        let bottom = rect.y.saturating_add(rect.height).min(info.height);
        if left >= right || top >= bottom {
            return;
        }

        if info.format == ColorFormat::ARGB8888 {
            let pitch = info.pitch() as usize;
            let px = color.raw_value().to_ne_bytes();
            let mut fb = self.fb();
            let buf: &mut [u8] = &mut fb;
            for y in top..bottom {
                let mut off = y as usize * pitch + left as usize * 4;
                let end = y as usize * pitch + right as usize * 4;
                if end > buf.len() {
                    break;
                }
                while off < end {
                    buf[off..off + 4].copy_from_slice(&px);
                    off += 4;
                }
            }
        } else {
            for j in top..bottom {
                for i in left..right {
                    self.draw_pixel(i, j, color);
                }
            }
        }
    }

    /// Copy a rectangle within the framebuffer (`memmove` semantics).
    ///
    /// This is the primitive behind console scrolling. The generic version does
    /// a per-row copy honoring the framebuffer pitch and the vertical overlap
    /// direction. Crucially it never reads back already-displayed pixels through
    /// a slow GPU aperture more than the move strictly requires. Drivers with a
    /// hardware 2D blit engine can override this.
    fn copy_rect(&self, src_x: u32, src_y: u32, dst_x: u32, dst_y: u32, width: u32, height: u32) {
        let info = self.info();
        let w = width
            .min(info.width.saturating_sub(src_x))
            .min(info.width.saturating_sub(dst_x)) as usize;
        let h = height
            .min(info.height.saturating_sub(src_y))
            .min(info.height.saturating_sub(dst_y)) as usize;
        if w == 0 || h == 0 {
            return;
        }
        let pitch = info.pitch() as usize;
        let bpp = info.format.bytes() as usize;
        let row_bytes = w * bpp;
        let mut fb = self.fb();
        let buf: &mut [u8] = &mut fb;

        let mut copy_row = |r: usize| {
            let s = (src_y as usize + r) * pitch + src_x as usize * bpp;
            let d = (dst_y as usize + r) * pitch + dst_x as usize * bpp;
            if s + row_bytes <= buf.len() && d + row_bytes <= buf.len() {
                buf.copy_within(s..s + row_bytes, d);
            }
        };

        if dst_y > src_y {
            for r in (0..h).rev() {
                copy_row(r);
            }
        } else {
            for r in 0..h {
                copy_row(r);
            }
        }
    }

    /// Blit a CPU-side ARGB8888 buffer into the framebuffer at `(dst_x, dst_y)`.
    ///
    /// `src` is row-major with `src_stride` pixels per row; only the top-left
    /// `width` x `height` window is used. This is the workhorse of the
    /// double-buffered console: drawing happens in cached RAM and the dirty
    /// region is pushed here in bulk. The generic version copies whole rows at a
    /// time (a single `copy_from_slice` per row for ARGB8888), which is friendly
    /// to write-combining GPU memory. Drivers may override to use DMA / a copy
    /// engine.
    fn blit_from(
        &self,
        dst_x: u32,
        dst_y: u32,
        src: &[u32],
        src_stride: usize,
        width: u32,
        height: u32,
    ) {
        let info = self.info();
        let w = width.min(info.width.saturating_sub(dst_x)) as usize;
        let h = height.min(info.height.saturating_sub(dst_y)) as usize;
        if w == 0 || h == 0 || src_stride == 0 {
            return;
        }
        let pitch = info.pitch() as usize;

        if info.format == ColorFormat::ARGB8888 {
            let mut fb = self.fb();
            let buf: &mut [u8] = &mut fb;
            for r in 0..h {
                let src_off = r * src_stride;
                if src_off + w > src.len() {
                    break;
                }
                let src_bytes = unsafe {
                    core::slice::from_raw_parts(src[src_off..].as_ptr() as *const u8, w * 4)
                };
                let d = (dst_y as usize + r) * pitch + dst_x as usize * 4;
                let d_end = d + w * 4;
                if d_end > buf.len() {
                    break;
                }
                buf[d..d_end].copy_from_slice(src_bytes);
            }
        } else {
            for r in 0..h {
                let src_off = r * src_stride;
                if src_off + w > src.len() {
                    break;
                }
                for c in 0..w {
                    self.draw_pixel(
                        dst_x + c as u32,
                        dst_y + r as u32,
                        RgbColor(src[src_off + c] & 0x00FF_FFFF),
                    );
                }
            }
        }
    }

    /// Alpha-composite a premultiplied ARGB8888 source "over" the framebuffer at
    /// `(dst_x, dst_y)`, which may be negative (the source is clipped to the
    /// visible area). This is the kernel-composited hardware cursor: wlroots is
    /// forced onto the legacy KMS path, so it hands us the pointer bitmap via
    /// `DRM_IOCTL_MODE_CURSOR` and we draw it on top of every scanned-out frame
    /// instead of the compositor re-rendering the whole scene on each move.
    ///
    /// wlroots renders cursors with premultiplied alpha, so the "over" operator
    /// is `out = src + dst * (255 - a) / 255`. Fully transparent pixels are
    /// skipped and fully opaque pixels are copied without reading the (slow,
    /// PCIe-mapped) destination — so only the antialiased edge pays for a
    /// read-modify-write.
    fn blit_argb_over(
        &self,
        dst_x: i32,
        dst_y: i32,
        src: &[u32],
        src_stride: usize,
        width: u32,
        height: u32,
    ) {
        let info = self.info();
        if info.format != ColorFormat::ARGB8888 || src_stride == 0 {
            return;
        }
        let pitch = info.pitch() as usize;
        let (fw, fh) = (info.width as i32, info.height as i32);
        let mut fb = self.fb();
        let buf: &mut [u8] = &mut fb;
        for r in 0..height as i32 {
            let py = dst_y + r;
            if py < 0 || py >= fh {
                continue;
            }
            let src_row = r as usize * src_stride;
            for c in 0..width as i32 {
                let px = dst_x + c;
                if px < 0 || px >= fw {
                    continue;
                }
                let si = src_row + c as usize;
                if si >= src.len() {
                    break;
                }
                let s = src[si];
                let a = s >> 24;
                if a == 0 {
                    continue;
                }
                let d_off = py as usize * pitch + px as usize * 4;
                if d_off + 4 > buf.len() {
                    continue;
                }
                let out = if a == 0xff {
                    s & 0x00FF_FFFF
                } else {
                    let inv = 255 - a;
                    let (sr, sg, sb) = ((s >> 16) & 0xff, (s >> 8) & 0xff, s & 0xff);
                    let d = u32::from_ne_bytes([buf[d_off], buf[d_off + 1], buf[d_off + 2], 0]);
                    let (dr, dg, db) = ((d >> 16) & 0xff, (d >> 8) & 0xff, d & 0xff);
                    // premultiplied over; clamp guards against a non-premultiplied
                    // source (would otherwise bleed into the next channel).
                    let or = (sr + dr * inv / 255).min(0xff);
                    let og = (sg + dg * inv / 255).min(0xff);
                    let ob = (sb + db * inv / 255).min(0xff);
                    (or << 16) | (og << 8) | ob
                };
                buf[d_off..d_off + 4].copy_from_slice(&out.to_ne_bytes());
            }
        }
    }

    /// Clear the screen with `color`.
    fn clear(&self, color: RgbColor) {
        let info = self.info();
        self.fill_rect(
            &Rectangle {
                x: 0,
                y: 0,
                width: info.width,
                height: info.height,
            },
            color,
        )
    }

    /// Whether need to flush the frambuffer to screen.
    #[inline]
    fn need_flush(&self) -> bool {
        false
    }

    /// Flush framebuffer to screen.
    #[inline]
    fn flush(&self) -> DeviceResult {
        Ok(())
    }
}

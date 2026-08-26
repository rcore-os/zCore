//! Software drawing for lunarbar, built on two pure-Rust crates that keep the
//! binary a static musl executable with zero system dependencies:
//!
//! - **tiny-skia** rasterises every shape with real anti-aliasing: the rounded
//!   pills, the ◑/☾ launcher discs (vector paths + mask, no hand-rolled
//!   coverage math), the ▼/▲ triangles, and the load gauges — which get a
//!   green→amber→red linear gradient for free.
//! - **embedded-graphics** supplies the text: mature ISO-8859-1 bitmap fonts
//!   (FONT_9X15 + real bold), so lowercase, accents and eñes in window titles
//!   render properly. tiny-skia has no text support; e-g has no AA shapes —
//!   together they cover each other's blind spot.
//!
//! `Canvas` owns an RGBA tiny-skia `Pixmap`; bars draw into it and then
//! `blit_xrgb` swizzles the finished frame into the wl_shm XRGB8888 buffer.

use embedded_graphics::{
    mono_font::{
        iso_8859_1::{FONT_9X15, FONT_9X15_BOLD},
        MonoTextStyle,
    },
    pixelcolor::Rgb888,
    prelude::*,
    text::{Baseline, Text},
};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap,
    PixmapPaint, Rect, SpreadMode, Stroke, Transform,
};

pub type Rgb = (u8, u8, u8);

/// Glyph cell height of the bar font (FONT_9X15).
pub const GLYPH_H: i32 = 15;
/// Glyph advance (cell width) of the bar font.
pub const GLYPH_W: i32 = 9;

/// Cubic-Bézier circle constant (approximates a 90° arc).
const K: f32 = 0.552_284_8;

#[inline]
fn color(c: Rgb, a: f32) -> Color {
    Color::from_rgba8(c.0, c.1, c.2, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// An RGBA scratch frame with AA vector drawing (tiny-skia) and bitmap text
/// (embedded-graphics), blitted to XRGB8888 when the frame is complete.
pub struct Canvas {
    pix: Pixmap,
}

impl Canvas {
    /// Fallible constructor — OOM / absurd sizes must not abort the panel
    /// (`panic = "abort"` in release).
    pub fn try_new(w: usize, h: usize) -> Option<Self> {
        let pix = Pixmap::new(w.max(1) as u32, h.max(1) as u32)?;
        Some(Self { pix })
    }

    pub fn new(w: usize, h: usize) -> Self {
        Self::try_new(w, h).expect("pixmap alloc")
    }

    #[allow(dead_code)]
    pub fn width(&self) -> u32 {
        self.pix.width()
    }

    #[allow(dead_code)]
    pub fn height(&self) -> u32 {
        self.pix.height()
    }

    fn paint<'a>(c: Rgb, a: f32) -> Paint<'a> {
        let mut p = Paint::default();
        p.set_color(color(c, a));
        p.anti_alias = true;
        p
    }

    fn fill(&mut self, path: &Path, c: Rgb, a: f32) {
        self.pix
            .fill_path(path, &Self::paint(c, a), FillRule::Winding, Transform::identity(), None);
    }

    /// Fill the whole canvas with a solid colour.
    pub fn clear(&mut self, c: Rgb) {
        self.pix.fill(color(c, 1.0));
    }

    /// Horizontal 1px line (used for the bars' accent rules).
    pub fn hline(&mut self, x0: i32, y: i32, len: i32, c: Rgb, a: f32) {
        if let Some(r) = Rect::from_xywh(x0 as f32, y as f32, len.max(0) as f32, 1.0) {
            self.fill(&PathBuilder::from_rect(r), c, a);
        }
    }

    /// A faint vertical separator line, centred in a bar of height `h`, ~half
    /// the bar tall. Cleaner than a dot for grouping modules.
    pub fn vrule(&mut self, x: i32, h: i32, c: Rgb) {
        let y0 = (h / 4) as f32;
        let y1 = (h - h / 4) as f32;
        if let Some(r) = Rect::from_xywh(x as f32, y0, 1.0, y1 - y0) {
            self.fill(&PathBuilder::from_rect(r), c, 0.30);
        }
    }

    /// A filled rounded rectangle, corner radius `rad`. Used for the clock and
    /// date pills and the active taskbar button, matching waybar's
    /// `border-radius: 6px` — now genuinely round thanks to tiny-skia's AA.
    pub fn round_rect(&mut self, x: i32, y: i32, rw: i32, rh: i32, rad: i32, c: Rgb) {
        if let Some(p) = rounded_rect_path(x as f32, y as f32, rw as f32, rh as f32, rad as f32) {
            self.fill(&p, c, 1.0);
        }
    }

    /// A small solid triangle in an `s`x`s` box at (x,y). `up=true` points up
    /// (tip at top → upload), else down (tip at bottom → download).
    pub fn triangle(&mut self, x: i32, y: i32, s: i32, up: bool, c: Rgb) {
        let (x, y, s) = (x as f32, y as f32, s as f32);
        let mut pb = PathBuilder::new();
        if up {
            pb.move_to(x + s / 2.0, y);
            pb.line_to(x + s, y + s);
            pb.line_to(x, y + s);
        } else {
            pb.move_to(x, y);
            pb.line_to(x + s, y);
            pb.line_to(x + s / 2.0, y + s);
        }
        pb.close();
        if let Some(p) = pb.finish() {
            self.fill(&p, c, 1.0);
        }
    }

    /// A small solid triangle pointing left/right in an `s`x`s` box at (x,y) —
    /// the calendar's month-nav arrows.
    pub fn triangle_h(&mut self, x: i32, y: i32, s: i32, left: bool, c: Rgb) {
        let (x, y, s) = (x as f32, y as f32, s as f32);
        let mut pb = PathBuilder::new();
        if left {
            pb.move_to(x + s, y);
            pb.line_to(x + s, y + s);
            pb.line_to(x, y + s / 2.0);
        } else {
            pb.move_to(x, y);
            pb.line_to(x, y + s);
            pb.line_to(x + s, y + s / 2.0);
        }
        pb.close();
        if let Some(p) = pb.finish() {
            self.fill(&p, c, 1.0);
        }
    }

    /// A mini horizontal gauge: a pill-shaped dark track filled `frac` (0..1)
    /// of its width with a green→amber→red gradient (foot-terminal palette),
    /// so a busy metric reads at a glance.
    pub fn gauge(&mut self, x: i32, y: i32, gw: i32, gh: i32, frac: f32, track: Rgb) {
        let frac = frac.clamp(0.0, 1.0);
        let rad = gh / 2;
        self.round_rect(x, y, gw, gh, rad, track);
        let filled = (gw as f32 * frac).round();
        if filled < 1.0 {
            return;
        }
        let Some(p) = rounded_rect_path(x as f32, y as f32, filled, gh as f32, rad as f32) else {
            return;
        };
        // Gradient spans the FULL track, revealed by the fill width, so the
        // visible leading edge carries the colour of the current level.
        let stops = vec![
            GradientStop::new(0.0, Color::from_rgba8(0x8f, 0xd1, 0x8a, 0xff)), // green
            GradientStop::new(0.5, Color::from_rgba8(0xe0, 0xc0, 0x7a, 0xff)), // amber
            GradientStop::new(1.0, Color::from_rgba8(0xe0, 0x7a, 0x7a, 0xff)), // red
        ];
        if let Some(shader) = LinearGradient::new(
            tiny_skia::Point::from_xy(x as f32, y as f32),
            tiny_skia::Point::from_xy((x + gw) as f32, y as f32),
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            let mut paint = Paint::default();
            paint.shader = shader;
            paint.anti_alias = true;
            self.pix
                .fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    /// The ◑ launcher: an outlined circle whose right half is filled. Matches
    /// the waybar `custom/launcher` glyph.
    pub fn disc_half(&mut self, x: i32, y: i32, d: i32, c: Rgb) {
        let r = d as f32 / 2.0;
        let cx = x as f32 + r;
        let cy = y as f32 + r;
        // Outer ring.
        if let Some(circle) = PathBuilder::from_circle(cx, cy, r - 0.75) {
            self.pix.stroke_path(
                &circle,
                &Self::paint(c, 1.0),
                &Stroke {
                    width: 1.5,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
        // Right-half fill: a semicircle built from two quarter-arc cubics.
        let ri = r - 0.5;
        let mut pb = PathBuilder::new();
        pb.move_to(cx, cy - ri);
        pb.cubic_to(cx + K * ri, cy - ri, cx + ri, cy - K * ri, cx + ri, cy);
        pb.cubic_to(cx + ri, cy + K * ri, cx + K * ri, cy + ri, cx, cy + ri);
        pb.close();
        if let Some(p) = pb.finish() {
            self.fill(&p, c, 1.0);
        }
    }

    /// The ☾ crescent: the sun disc masked by an offset moon disc — done with
    /// a real inverted clip mask instead of hand-rolled coverage math.
    /// Mirrors lunarbg's eclipse crescent so the top bar matches the wallpaper.
    pub fn crescent(&mut self, x: i32, y: i32, d: i32, c: Rgb) {
        if d <= 0 {
            return;
        }
        // Rasterise into a d×d scratch pixmap, not against a full-canvas mask:
        // the app menu draws this on an output-sized canvas, where a
        // screen-sized Mask would mean a multi-megabyte alloc plus two
        // full-screen passes (fill + invert) for an 18px glyph, on every
        // repaint.
        let s = d as u32;
        let r = d as f32 / 2.0;
        let (cx, cy) = (r, r);
        let (mx, my, mr) = (cx + r * 0.42, cy - r * 0.10, r * 0.92);
        let (Some(sun), Some(moon)) = (
            PathBuilder::from_circle(cx, cy, r),
            PathBuilder::from_circle(mx, my, mr),
        ) else {
            return;
        };
        let (Some(mut glyph), Some(mut mask)) = (Pixmap::new(s, s), Mask::new(s, s)) else {
            return;
        };
        mask.fill_path(&moon, FillRule::Winding, true, Transform::identity());
        mask.invert();
        glyph.fill_path(
            &sun,
            &Self::paint(c, 1.0),
            FillRule::Winding,
            Transform::identity(),
            Some(&mask),
        );
        self.pixmap(x, y, &glyph);
    }

    /// Bright accent indicator line at the bottom edge of an active taskbar button.
    pub fn active_line(&mut self, x: i32, y: i32, w: i32, c: Rgb) {
        if let Some(p) = rounded_rect_path((x + 4) as f32, y as f32, (w - 8).max(4) as f32, 2.0, 1.0) {
            self.fill(&p, c, 1.0);
        }
    }

    /// Vector power symbol (⏻): circle arc + top vertical line in white.
    pub fn power_icon(&mut self, x: i32, y: i32, s: i32, c: Rgb) {
        let (x, y, s) = (x as f32, y as f32, s as f32);
        let r = s / 2.0 - 1.0;
        let cx = x + s / 2.0;
        let cy = y + s / 2.0;
        let stroke = Stroke { width: 1.8, ..Stroke::default() };
        // Circle arc
        let mut pb = PathBuilder::new();
        pb.move_to(cx + r * 0.5, cy - r * 0.866);
        pb.cubic_to(cx + r * 1.2, cy, cx + r * 0.5, cy + r * 1.1, cx, cy + r);
        pb.cubic_to(cx - r * 0.5, cy + r * 1.1, cx - r * 1.2, cy, cx - r * 0.5, cy - r * 0.866);
        if let Some(p) = pb.finish() {
            self.pix.stroke_path(&p, &Self::paint(c, 1.0), &stroke, Transform::identity(), None);
        }
        // Top vertical bar
        let mut pb2 = PathBuilder::new();
        pb2.move_to(cx, cy - r * 1.1);
        pb2.line_to(cx, cy - r * 0.1);
        if let Some(p) = pb2.finish() {
            self.pix.stroke_path(&p, &Self::paint(c, 1.0), &stroke, Transform::identity(), None);
        }
    }

    /// Vector speaker icon (🔊 / 🔇)
    pub fn volume_icon(&mut self, x: i32, y: i32, s: i32, c: Rgb, muted: bool) {
        let (x, y, s) = (x as f32, y as f32, s as f32);
        let stroke = Stroke { width: 1.5, ..Stroke::default() };
        // Speaker box + cone
        let mut pb = PathBuilder::new();
        pb.move_to(x, y + s * 0.35);
        pb.line_to(x + s * 0.25, y + s * 0.35);
        pb.line_to(x + s * 0.5, y + s * 0.15);
        pb.line_to(x + s * 0.5, y + s * 0.85);
        pb.line_to(x + s * 0.25, y + s * 0.65);
        pb.line_to(x, y + s * 0.65);
        pb.close();
        if let Some(p) = pb.finish() {
            self.fill(&p, c, 1.0);
        }
        if muted {
            // X mark
            let mut pb_x = PathBuilder::new();
            pb_x.move_to(x + s * 0.65, y + s * 0.35);
            pb_x.line_to(x + s * 0.95, y + s * 0.65);
            pb_x.move_to(x + s * 0.95, y + s * 0.35);
            pb_x.line_to(x + s * 0.65, y + s * 0.65);
            if let Some(p) = pb_x.finish() {
                self.pix.stroke_path(&p, &Self::paint(c, 1.0), &stroke, Transform::identity(), None);
            }
        } else {
            // Sound wave arcs
            let mut pb_wave = PathBuilder::new();
            pb_wave.move_to(x + s * 0.65, y + s * 0.35);
            pb_wave.cubic_to(x + s * 0.8, y + s * 0.45, x + s * 0.8, y + s * 0.55, x + s * 0.65, y + s * 0.65);
            if let Some(p) = pb_wave.finish() {
                self.pix.stroke_path(&p, &Self::paint(c, 1.0), &stroke, Transform::identity(), None);
            }
        }
    }

    /// Vector padlock icon (🔒)
    pub fn lock_icon(&mut self, x: i32, y: i32, s: i32, c: Rgb) {
        let (x, y, s) = (x as f32, y as f32, s as f32);
        let stroke = Stroke { width: 1.5, ..Stroke::default() };
        let mut pb = PathBuilder::new();
        pb.move_to(x + s * 0.3, y + s * 0.45);
        pb.line_to(x + s * 0.3, y + s * 0.25);
        pb.cubic_to(x + s * 0.3, y + s * 0.05, x + s * 0.7, y + s * 0.05, x + s * 0.7, y + s * 0.25);
        pb.line_to(x + s * 0.7, y + s * 0.45);
        if let Some(p) = pb.finish() {
            self.pix.stroke_path(&p, &Self::paint(c, 1.0), &stroke, Transform::identity(), None);
        }
        if let Some(p) = rounded_rect_path(x + s * 0.2, y + s * 0.45, s * 0.6, s * 0.5, 2.0) {
            self.fill(&p, c, 1.0);
        }
    }

    /// Vector logout icon (🚪 / ➔)
    pub fn exit_icon(&mut self, x: i32, y: i32, s: i32, c: Rgb) {
        let (x, y, s) = (x as f32, y as f32, s as f32);
        let stroke = Stroke { width: 1.5, ..Stroke::default() };
        let mut pb = PathBuilder::new();
        pb.move_to(x + s * 0.5, y + s * 0.15);
        pb.line_to(x + s * 0.15, y + s * 0.15);
        pb.line_to(x + s * 0.15, y + s * 0.85);
        pb.line_to(x + s * 0.5, y + s * 0.85);
        if let Some(p) = pb.finish() {
            self.pix.stroke_path(&p, &Self::paint(c, 1.0), &stroke, Transform::identity(), None);
        }
        let mut pb_arr = PathBuilder::new();
        pb_arr.move_to(x + s * 0.35, y + s * 0.5);
        pb_arr.line_to(x + s * 0.85, y + s * 0.5);
        pb_arr.move_to(x + s * 0.65, y + s * 0.3);
        pb_arr.line_to(x + s * 0.85, y + s * 0.5);
        pb_arr.line_to(x + s * 0.65, y + s * 0.7);
        if let Some(p) = pb_arr.finish() {
            self.pix.stroke_path(&p, &Self::paint(c, 1.0), &stroke, Transform::identity(), None);
        }
    }

    /// Vector reboot icon (🔄)
    pub fn reboot_icon(&mut self, x: i32, y: i32, s: i32, c: Rgb) {
        let (x, y, s) = (x as f32, y as f32, s as f32);
        let stroke = Stroke { width: 1.5, ..Stroke::default() };
        let cx = x + s / 2.0;
        let cy = y + s / 2.0;
        let r = s * 0.35;
        let mut pb = PathBuilder::new();
        pb.move_to(cx + r, cy);
        pb.cubic_to(cx + r, cy + r * 1.2, cx - r * 1.2, cy + r, cx - r, cy);
        pb.cubic_to(cx - r, cy - r * 1.2, cx + r * 0.8, cy - r, cx + r * 0.7, cy - r * 0.3);
        if let Some(p) = pb.finish() {
            self.pix.stroke_path(&p, &Self::paint(c, 1.0), &stroke, Transform::identity(), None);
        }
        let mut pb_ah = PathBuilder::new();
        pb_ah.move_to(cx + r * 0.3, cy - r * 0.6);
        pb_ah.line_to(cx + r * 0.7, cy - r * 0.3);
        pb_ah.line_to(cx + r * 0.9, cy - r * 0.7);
        if let Some(p) = pb_ah.finish() {
            self.fill(&p, c, 1.0);
        }
    }

    /// Draw a left-aligned string (FONT_9X15, transparent background) with its
    /// cell top at `y`. Returns the advance in pixels.
    pub fn text(&mut self, s: &str, x: i32, y: i32, c: Rgb) -> i32 {
        let style = MonoTextStyle::new(&FONT_9X15, Rgb888::new(c.0, c.1, c.2));
        Text::with_baseline(s, Point::new(x, y), style, Baseline::Top)
            .draw(self)
            .map(|end| end.x - x)
            .unwrap_or(0)
    }

    /// Bold variant of `text` (FONT_9X15_BOLD, same metrics). Matches waybar's
    /// `font-weight: bold` clock.
    pub fn text_bold(&mut self, s: &str, x: i32, y: i32, c: Rgb) -> i32 {
        let style = MonoTextStyle::new(&FONT_9X15_BOLD, Rgb888::new(c.0, c.1, c.2));
        Text::with_baseline(s, Point::new(x, y), style, Baseline::Top)
            .draw(self)
            .map(|end| end.x - x)
            .unwrap_or(0)
    }

    /// Pixel width a string will occupy (monospace: chars × cell width).
    pub fn text_width(s: &str) -> i32 {
        s.chars().count() as i32 * GLYPH_W
    }

    /// Alpha-blend a pre-scaled icon pixmap at (x,y) — the taskbar/menu icon
    /// slot (icons::IconCache hands out pixmaps already at slot size).
    pub fn pixmap(&mut self, x: i32, y: i32, pm: &Pixmap) {
        self.pix.draw_pixmap(
            x,
            y,
            pm.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Letter fallback for a missing icon: a rounded square with the app's
    /// bold initial, so every button/row keeps a consistent icon slot.
    pub fn badge(&mut self, x: i32, y: i32, s: i32, ch: char, bg: Rgb, fg: Rgb) {
        self.round_rect(x, y, s, s, (s / 4).max(3), bg);
        let tx = x + (s - GLYPH_W) / 2 + 1;
        let ty = y + (s - GLYPH_H) / 2;
        let up: String = ch.to_uppercase().take(1).collect();
        self.text_bold(&up, tx, ty, fg);
    }

    /// Swizzle the finished RGBA frame into an XRGB8888 (B,G,R,X) buffer.
    /// `dst` must be at least w*h*4 bytes. Returns false if sizes disagree
    /// (never panics — the panel must stay up under a bad configure).
    pub fn blit_xrgb(&self, dst: &mut [u8]) -> bool {
        let src = self.pix.data();
        let w = self.pix.width() as usize;
        let h = self.pix.height() as usize;
        let n = w * h;
        if dst.len() < n * 4 || src.len() < n * 4 {
            return false;
        }
        // Full-surface swizzle; see blit_argb. The size gate keeps the thin
        // status bars serial and lets the big preview/menu blits fan out.
        crate::par::par_rows(&mut dst[..n * 4], h, w * 4, |y0, band| {
            for (ry, row) in band.chunks_mut(w * 4).enumerate() {
                let base = (y0 + ry) * w * 4;
                for x in 0..w {
                    let o = x * 4;
                    let s = base + o;
                    // Everything drawn is opaque (the bar clears to a solid
                    // ground), so premultiplied RGBA here equals straight RGB.
                    row[o] = src[s + 2]; // B
                    row[o + 1] = src[s + 1]; // G
                    row[o + 2] = src[s]; // R
                    row[o + 3] = 0xff; // X
                }
            }
        });
        true
    }

    /// Nearest-neighbour upscale from the logical canvas into a
    /// `scale`-times-larger XRGB buffer (HiDPI without rewriting every draw).
    pub fn blit_xrgb_scaled(&self, dst: &mut [u8], scale: u32) -> bool {
        let scale = scale.max(1) as usize;
        if scale == 1 {
            return self.blit_xrgb(dst);
        }
        let src = self.pix.data();
        let w = self.pix.width() as usize;
        let h = self.pix.height() as usize;
        let bw = w * scale;
        let bh = h * scale;
        if dst.len() < bw * bh * 4 || src.len() < w * h * 4 {
            return false;
        }
        for y in 0..bh {
            let sy = y / scale;
            for x in 0..bw {
                let sx = x / scale;
                let s = (sy * w + sx) * 4;
                let o = (y * bw + x) * 4;
                dst[o] = src[s + 2];
                dst[o + 1] = src[s + 1];
                dst[o + 2] = src[s];
                dst[o + 3] = 0xff;
            }
        }
        true
    }

    /// Swizzle the finished RGBA frame into an ARGB8888 (B,G,R,A) buffer,
    /// preserving alpha. tiny-skia stores premultiplied RGBA and wl_shm's
    /// Argb8888 also expects premultiplied, so the bytes map straight across.
    /// Used by the translucent menu overlay.
    pub fn blit_argb(&self, dst: &mut [u8]) -> bool {
        let src = self.pix.data();
        let w = self.pix.width() as usize;
        let h = self.pix.height() as usize;
        let n = w * h;
        if dst.len() < n * 4 || src.len() < n * 4 {
            return false;
        }
        // Full-surface swizzle: split by band, read `src` (immutable) by
        // absolute offset. Byte-identical to the serial copy; the size gate in
        // `par_rows` keeps the thin bars serial and only the full-output menu
        // fans out.
        crate::par::par_rows(&mut dst[..n * 4], h, w * 4, |y0, band| {
            for (ry, row) in band.chunks_mut(w * 4).enumerate() {
                let base = (y0 + ry) * w * 4;
                for x in 0..w {
                    let o = x * 4;
                    let s = base + o;
                    row[o] = src[s + 2]; // B
                    row[o + 1] = src[s + 1]; // G
                    row[o + 2] = src[s]; // R
                    row[o + 3] = src[s + 3]; // A
                }
            }
        });
        true
    }

    #[allow(dead_code)]
    /// Nearest-neighbour upscale into a `scale`-times-larger ARGB buffer.
    pub fn blit_argb_scaled(&self, dst: &mut [u8], scale: u32) -> bool {
        let scale = scale.max(1) as usize;
        if scale == 1 {
            return self.blit_argb(dst);
        }
        let src = self.pix.data();
        let w = self.pix.width() as usize;
        let h = self.pix.height() as usize;
        let bw = w * scale;
        let bh = h * scale;
        if dst.len() < bw * bh * 4 || src.len() < w * h * 4 {
            return false;
        }
        for y in 0..bh {
            let sy = y / scale;
            for x in 0..bw {
                let sx = x / scale;
                let s = (sy * w + sx) * 4;
                let o = (y * bw + x) * 4;
                dst[o] = src[s + 2];
                dst[o + 1] = src[s + 1];
                dst[o + 2] = src[s];
                dst[o + 3] = src[s + 3];
            }
        }
        true
    }

    /// Draw a filled path at the given RGB with explicit alpha — for the
    /// menu's translucent scrim and hover highlights.
    pub fn fill_rect_a(&mut self, x: i32, y: i32, rw: i32, rh: i32, c: Rgb, a: f32) {
        if let Some(r) = Rect::from_xywh(x as f32, y as f32, rw.max(0) as f32, rh.max(0) as f32) {
            self.fill(&PathBuilder::from_rect(r), c, a);
        }
    }

    /// Filled rounded rect with explicit alpha.
    pub fn round_rect_a(&mut self, x: i32, y: i32, rw: i32, rh: i32, rad: i32, c: Rgb, a: f32) {
        if let Some(p) = rounded_rect_path(x as f32, y as f32, rw as f32, rh as f32, rad as f32) {
            self.fill(&p, c, a);
        }
    }
}

// embedded-graphics draw target: text renders straight into the RGBA pixmap.
impl OriginDimensions for Canvas {
    fn size(&self) -> Size {
        Size::new(self.pix.width(), self.pix.height())
    }
}

impl DrawTarget for Canvas {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let w = self.pix.width() as i32;
        let h = self.pix.height() as i32;
        let data = self.pix.data_mut();
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && p.x < w && p.y < h {
                let i = ((p.y * w + p.x) as usize) * 4;
                // Pixmap is premultiplied RGBA; alpha 255 makes this straight.
                data[i] = c.r();
                data[i + 1] = c.g();
                data[i + 2] = c.b();
                data[i + 3] = 0xff;
            }
        }
        Ok(())
    }
}

/// A rounded-rect path with all corners of radius `r` (quarter-arc cubics).
fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.clamp(0.0, (w / 2.0).min(h / 2.0));
    if r <= 0.5 {
        return Rect::from_xywh(x, y, w, h).map(PathBuilder::from_rect);
    }
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + K * r, y, x + w, y + r - K * r, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + K * r, x + w - r + K * r, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - K * r, y + h, x, y + h - r + K * r, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - K * r, x + r - K * r, y, x + r, y);
    pb.close();
    pb.finish()
}

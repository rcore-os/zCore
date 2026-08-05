//! The Eclipse OS animated cosmic background, ported from the original
//! smithay compositor (eclipse-old: `sidewind/src/ui.rs`).
//!
//! Static base (rendered once per size):
//! - vertical cosmic gradient, COSMIC_DEEP -> COSMIC_MID, with a soft cyan
//!   nebula glow behind the logo;
//! - deterministic starfield scaled to the output area;
//! - the 48 px blueprint grid, rgb(18,28,55).
//!
//! Animated logo (redrawn every frame inside [`Layout::region`]):
//! - the eclipse crescent: a golden sun disc masked by an offset moon circle
//!   (mask offset `(r/4, -r/5)`, moon radius `9r/10` — the mask shows the
//!   cosmic background through it);
//! - the orbiting text ring "ECLIPSE-SYSTEM-KERNEL-…" (upright characters
//!   with a dark outline, as in the original);
//! - three tech arcs rotating at different speeds and directions;
//! - five pulsing concentric rings;
//! - technical ticks every 5° (major every 30°) with shimmering brightness;
//! - the "ECLIPSE OS" wordmark under the crescent.
//!
//! All radii come from the original design (crescent 140, text ring 165,
//! arcs 145/180/195, ticks 230..255, rings 240..280 — on a 280 px logo) and
//! scale with the output via the original sizing rule
//! `clamp(min(w,h)/2 - 120, 120, 280)`.

// ---------------------------------------------------------------- palette

const COSMIC_DEEP: Rgb = (2.0 / 255.0, 2.0 / 255.0, 8.0 / 255.0);
const COSMIC_MID: Rgb = (8.0 / 255.0, 15.0 / 255.0, 35.0 / 255.0);
const NEBULA_CYAN: Rgb = (0.0, 70.0 / 255.0, 110.0 / 255.0);
const GRID_BLUE: Rgb = (18.0 / 255.0, 28.0 / 255.0, 55.0 / 255.0);
const ACCENT_CYAN: Rgb = (0.0, 229.0 / 255.0, 1.0);
const ACCENT_VIOLET: Rgb = (180.0 / 255.0, 140.0 / 255.0, 1.0);
const GLOW_HI: Rgb = ACCENT_CYAN;
const GLOW_MID: Rgb = (0.0, 128.0 / 255.0, 160.0 / 255.0);
const GLOW_DIM: Rgb = (0.0, 64.0 / 255.0, 80.0 / 255.0);
const SUN_FILL: Rgb = (1.0, 220.0 / 255.0, 80.0 / 255.0);
const SUN_EDGE: Rgb = (1.0, 200.0 / 255.0, 50.0 / 255.0);

const TEXT_RING: &str = "ECLIPSE-SYSTEM-KERNEL-6.X-STABLE-LINK-ACTIVE-";

type Rgb = (f32, f32, f32);

// ---------------------------------------------------------------- layout

/// Placement of the animated logo on an output.
pub struct Layout {
    pub cx: f32,
    pub cy: f32,
    /// Scale relative to the original 280 px design.
    pub s: f32,
    /// Horizontal squeeze so circles LOOK circular on the monitor.
    ///
    /// The mode the driver sets (synthetic KMS / GOP, or the NVIDIA KMS
    /// driver) is often NOT the panel's native aspect (e.g. a 4:3 1024x768
    /// mode on a 16:9 panel); the panel then stretches the framebuffer and
    /// every circle shows as an ellipse. We pre-squeeze the logo by
    /// fb_aspect/monitor_aspect so the panel's stretch cancels out.
    ///
    /// The monitor aspect is taken, in order of preference, from: the panel's
    /// physical size reported in `wl_output.geometry` (fully automatic — see
    /// `monitor_aspect` in main.rs), then the `LUNARBG_ASPECT` env override
    /// ("16:9", "16:10" or a decimal like "1.778"), then 1.0 (draw round,
    /// e.g. QEMU where the mode already matches the virtual panel).
    pub sx: f32,
    /// (x, y, w, h) of the rect that the animation redraws each frame.
    pub region: (usize, usize, usize, usize),
}

fn monitor_aspect_from_env() -> Option<f32> {
    let v = std::env::var("LUNARBG_ASPECT").ok()?;
    let v = v.trim();
    let aspect = if let Some((a, b)) = v.split_once(':') {
        a.trim().parse::<f32>().ok()? / b.trim().parse::<f32>().ok()?
    } else {
        v.parse::<f32>().ok()?
    };
    (aspect.is_finite() && aspect > 0.1).then_some(aspect)
}

pub fn layout(w: usize, h: usize, monitor_aspect: Option<f32>) -> Layout {
    let logo_r = ((w.min(h) as f32 / 2.0) - 120.0).clamp(120.0, 280.0);
    let s = logo_r / 280.0;
    let cx = w as f32 * 0.5;
    let cy = h as f32 * 0.46;
    let fb_aspect = w as f32 / h as f32;
    // Prefer the panel aspect detected from wl_output.geometry; fall back to
    // the LUNARBG_ASPECT override, then to 1.0 (no squeeze).
    let sx = monitor_aspect
        .filter(|a| a.is_finite() && *a > 0.1)
        .or_else(monitor_aspect_from_env)
        .map(|mon| (fb_aspect / mon).clamp(0.5, 1.5))
        .unwrap_or(1.0);
    // Outermost animated element: ring 280 + 5 px oscillation, plus the
    // wordmark below at 170 + text height. Take a comfortable margin.
    let reach = (300.0 * s).max(215.0 * s + 40.0) + 8.0;
    let x0 = ((cx - reach * sx).floor().max(0.0)) as usize;
    let y0 = ((cy - reach).floor().max(0.0)) as usize;
    let x1 = ((cx + reach * sx).ceil() as usize).min(w);
    let y1 = ((cy + reach).ceil() as usize).min(h);
    Layout {
        cx,
        cy,
        s,
        sx,
        region: (x0, y0, x1 - x0, y1 - y0),
    }
}

// ---------------------------------------------------------------- base

/// Render the static cosmic base as XRGB8888.
pub fn render_base(w: usize, h: usize, monitor_aspect: Option<f32>) -> Vec<u8> {
    let lay = layout(w, h, monitor_aspect);
    let mut buf = vec![0f32; w * h * 3];

    // Cosmic vertical gradient + nebula glow behind the logo.
    //
    // The gradient is a full-surface pass (one lerp per pixel), so it is split
    // across CPUs by horizontal band. Each band owns disjoint rows and writes
    // only its own slice; the output is identical to the serial loop.
    let fh = h as f32;
    crate::par::par_rows(&mut buf, h, w * 3, |y0, band| {
        for (ry, row) in band.chunks_mut(w * 3).enumerate() {
            let t = (y0 + ry) as f32 / fh;
            let (r, g, b) = lerp3(COSMIC_DEEP, COSMIC_MID, t);
            for x in 0..w {
                row[x * 3] = r;
                row[x * 3 + 1] = g;
                row[x * 3 + 2] = b;
            }
        }
    });
    // Soft radial nebula centred on the logo (squeezed like the logo so the
    // glow stays concentric with it on a stretching panel).
    let neb_r = 420.0 * lay.s + 120.0;
    let (nx0, nx1) = span(lay.cx, neb_r * lay.sx, w);
    let (ny0, ny1) = span(lay.cy, neb_r, h);
    for y in ny0..ny1 {
        for x in nx0..nx1 {
            let d = dist((x as f32 - lay.cx) / lay.sx + lay.cx, y as f32, lay.cx, lay.cy);
            if d < neb_r {
                let t = 1.0 - d / neb_r;
                let a = t * t * 0.22;
                let i = (y * w + x) * 3;
                buf[i] += NEBULA_CYAN.0 * a;
                buf[i + 1] += NEBULA_CYAN.1 * a;
                buf[i + 2] += NEBULA_CYAN.2 * a;
            }
        }
    }

    // Starfield, scaled to area.
    let count = ((w * h) as f32 / 6000.0) as u32;
    for i in 0..count {
        let x = (hash2(i, 1) % w as u32) as i32;
        let y = (hash2(i, 2) % h as u32) as i32;
        let bright = 0.25 + (hash2(i, 3) % 1000) as f32 / 1000.0 * 0.75;
        add_px_f(&mut buf, w, h, x, y, (bright, bright, bright * 0.95));
        if bright > 0.85 {
            let half = bright * 0.35;
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                add_px_f(&mut buf, w, h, x + dx, y + dy, (half, half, half));
            }
        }
    }

    // Blueprint grid, 48 px.
    const SPACING: usize = 48;
    for y in (0..h).step_by(SPACING) {
        for x in 0..w {
            blend_px_f(&mut buf, w, x, y, GRID_BLUE, 0.38);
        }
    }
    for x in (0..w).step_by(SPACING) {
        for y in 0..h {
            if y % SPACING != 0 {
                blend_px_f(&mut buf, w, x, y, GRID_BLUE, 0.38);
            }
        }
    }

    // Quantise to XRGB8888 with light dithering noise. Also a full-surface
    // pass: split `out` by band, read the float buffer by absolute pixel index.
    // The dither noise is a per-pixel hash of the byte offset, so bands compute
    // exactly the bytes the serial loop would — the image is identical.
    let mut out = vec![0u8; w * h * 4];
    let src: &[f32] = &buf;
    crate::par::par_rows(&mut out, h, w * 4, |y0, band| {
        for (ry, orow) in band.chunks_mut(w * 4).enumerate() {
            let y = y0 + ry;
            for x in 0..w {
                let px = y * w + x;
                let i = px * 3;
                let o = x * 4;
                let n = (hash2(i as u32, 0x9e37_79b9) as f32 / u32::MAX as f32 - 0.5) * 1.5;
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + n).round().clamp(0.0, 255.0) as u8;
                orow[o] = q(src[i + 2]);
                orow[o + 1] = q(src[i + 1]);
                orow[o + 2] = q(src[i]);
                orow[o + 3] = 0xff;
            }
        }
    });
    out
}

// ---------------------------------------------------------------- frame

/// Draw one animation frame: restore the logo region from `base`, then paint
/// the animated logo. `t_ms` is a monotonic millisecond clock; the original
/// compositor advanced `counter` once per ~60 Hz frame, so `counter =
/// t_ms * 0.06` reproduces its speeds.
pub fn render_frame(frame: &mut [u8], w: usize, base: &[u8], lay: &Layout, t_ms: u32) {
    let (rx, ry, rw, rh) = lay.region;
    for row in 0..rh {
        let off = ((ry + row) * w + rx) * 4;
        frame[off..off + rw * 4].copy_from_slice(&base[off..off + rw * 4]);
    }

    let mut pb = PixBuf {
        data: frame,
        w,
        clip: (rx, ry, rx + rw, ry + rh),
        sx: lay.sx,
    };
    let counter = t_ms as f32 * 0.06;
    let (cx, cy, s) = (lay.cx, lay.cy, lay.s);

    // --- five pulsing concentric rings (backmost) ---
    for (i, base_r) in [280.0f32, 275.0, 260.0, 255.0, 240.0].iter().enumerate() {
        let osc = (counter * (0.01 + i as f32 * 0.005)).sin() * 5.0;
        let r = (base_r + osc) * s;
        let color = if i % 2 == 0 { GLOW_DIM } else { ACCENT_VIOLET };
        let alpha = if i % 2 == 0 { 0.55 } else { 0.18 };
        pb.ring(cx, cy, r, 1.4, color, alpha);
    }

    // --- technical ticks every 5°, major every 30°, slow shimmer+drift ---
    let tick_phase = counter * 0.05; // degrees
    for angle in (0..360).step_by(5) {
        let is_major = angle % 30 == 0;
        let a = (angle as f32 + tick_phase).to_radians();
        let (r0, r1) = if is_major {
            (230.0 * s, 255.0 * s)
        } else {
            (235.0 * s, 250.0 * s)
        };
        let shimmer = (a * 2.0 + counter * 0.02).sin().abs();
        let (color, alpha) = if is_major {
            (ACCENT_CYAN, 0.25 + 0.45 * shimmer)
        } else {
            (GLOW_MID, 0.15 + 0.25 * shimmer)
        };
        let (sin, cos) = a.sin_cos();
        pb.line(
            cx + cos * r0 * pb.sx,
            cy + sin * r0,
            cx + cos * r1 * pb.sx,
            cy + sin * r1,
            1.2,
            color,
            alpha,
        );
    }

    // --- three tech arcs at different speeds/directions ---
    let arc_rot = counter * 0.5; // degrees
    pb.arc(cx, cy, 180.0 * s, -arc_rot * 1.5, 60.0, 2.0, GLOW_HI, 0.9);
    pb.arc(cx, cy, 195.0 * s, arc_rot * 0.8 + 180.0, 30.0, 2.0, ACCENT_VIOLET, 0.9);
    pb.arc(cx, cy, 145.0 * s, arc_rot * 1.2, 45.0, 2.0, ACCENT_CYAN, 0.9);

    // --- orbiting text ring (upright chars, dark outline) ---
    let chars: Vec<char> = TEXT_RING.chars().collect();
    let n = chars.len() as f32;
    let rot_phase = counter * 0.12; // degrees
    let text_r = 165.0 * s;
    let scale = ((2.0 * s).round() as usize).max(1);
    for (i, ch) in chars.iter().enumerate() {
        let a = ((i as f32 * 360.0 / n) + rot_phase).to_radians();
        let (sin, cos) = a.sin_cos();
        let gx = cx + cos * text_r * pb.sx;
        let gy = cy + sin * text_r;
        pb.glyph_outlined(*ch, gx, gy, scale, GLOW_HI, 0.85, COSMIC_DEEP);
    }

    // --- the eclipse crescent core ---
    let sun_r = 140.0 * s;
    let moon_r = sun_r * 9.0 / 10.0;
    // The moon-mask centre offset lives in the round pre-stretch space, so
    // its X component squeezes with everything else.
    let (mx, my) = (cx + sun_r / 4.0 * pb.sx, cy - sun_r / 5.0);
    let (sx0, sx1) = pb.clip_span_x(cx, (sun_r + 2.0) * pb.sx);
    let (sy0, sy1) = pb.clip_span_y(cy, sun_r + 2.0);
    for y in sy0..sy1 {
        for x in sx0..sx1 {
            let d = pb.edist(x as f32, y as f32, cx, cy);
            let cover = (sun_r - d + 0.5).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            // Moon mask: transparent, the cosmic base shows through.
            let dm = pb.edist(x as f32, y as f32, mx, my);
            let mask = (moon_r - dm + 0.5).clamp(0.0, 1.0);
            let a = cover * (1.0 - mask);
            if a <= 0.0 {
                continue;
            }
            // Edge tint on the outer 6 px of the sun.
            let edge = ((sun_r - d) / 6.0).clamp(0.0, 1.0);
            let color = lerp3(SUN_EDGE, SUN_FILL, edge);
            pb.blend(x, y, color, a);
        }
    }

    // --- "ECLIPSE OS" wordmark under the crescent ---
    let scale = ((3.0 * s).round() as usize).max(2);
    let text = "ECLIPSE OS";
    let advance = (6 * scale) as f32;
    let total = text.len() as f32 * advance - scale as f32;
    // Below the text ring (165) so the wordmark never collides with the
    // orbiting characters. The original drew it at +170, overlapping.
    let ty = cy + 215.0 * s;
    for (i, ch) in text.chars().enumerate() {
        let gx = cx - total / 2.0 + i as f32 * advance + advance / 2.0;
        pb.glyph_outlined(ch, gx, ty, scale, (0.90, 0.96, 1.0), 0.95, COSMIC_DEEP);
    }
}

// ------------------------------------------------------------- draw utils

struct PixBuf<'a> {
    data: &'a mut [u8],
    w: usize,
    /// (x0, y0, x1, y1) — drawing outside is discarded.
    clip: (usize, usize, usize, usize),
    /// Horizontal squeeze (see [`Layout::sx`]): circles are drawn as ellipses
    /// with X semi-axis `r * sx` so a stretching monitor shows them round.
    sx: f32,
}

impl PixBuf<'_> {
    fn blend(&mut self, x: usize, y: usize, c: Rgb, a: f32) {
        if x < self.clip.0 || y < self.clip.1 || x >= self.clip.2 || y >= self.clip.3 {
            return;
        }
        let i = (y * self.w + x) * 4;
        let a = a.clamp(0.0, 1.0);
        let mix = |old: u8, new: f32| -> u8 {
            (old as f32 * (1.0 - a) + new * 255.0 * a)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        self.data[i] = mix(self.data[i], c.2);
        self.data[i + 1] = mix(self.data[i + 1], c.1);
        self.data[i + 2] = mix(self.data[i + 2], c.0);
    }

    fn clip_span_x(&self, c: f32, r: f32) -> (usize, usize) {
        let lo = (c - r).floor().max(self.clip.0 as f32) as usize;
        let hi = (((c + r).ceil() as usize) + 1).min(self.clip.2);
        (lo, hi)
    }

    fn clip_span_y(&self, c: f32, r: f32) -> (usize, usize) {
        let lo = (c - r).floor().max(self.clip.1 as f32) as usize;
        let hi = (((c + r).ceil() as usize) + 1).min(self.clip.3);
        (lo, hi)
    }

    /// Undo the horizontal squeeze: distance is measured in the round,
    /// pre-stretch space so an on-screen ellipse reads as a circle.
    fn edist(&self, x: f32, y: f32, cx: f32, cy: f32) -> f32 {
        dist((x - cx) / self.sx + cx, y, cx, cy)
    }

    /// Thin anti-aliased ring.
    fn ring(&mut self, cx: f32, cy: f32, r: f32, thick: f32, c: Rgb, alpha: f32) {
        let (x0, x1) = self.clip_span_x(cx, (r + thick + 1.0) * self.sx);
        let (y0, y1) = self.clip_span_y(cy, r + thick + 1.0);
        let inner = r - thick;
        for y in y0..y1 {
            for x in x0..x1 {
                let d = self.edist(x as f32, y as f32, cx, cy);
                if d < inner - 1.0 || d > r + thick + 1.0 {
                    continue;
                }
                let cover = (thick / 2.0 - (d - (r - thick / 2.0)).abs() + 0.5).clamp(0.0, 1.0);
                if cover > 0.0 {
                    self.blend(x, y, c, alpha * cover);
                }
            }
        }
    }

    /// Anti-aliased thick line (capsule).
    fn line(&mut self, ax: f32, ay: f32, bx: f32, by: f32, thick: f32, c: Rgb, alpha: f32) {
        let half = thick / 2.0;
        let x0 = (ax.min(bx) - half - 1.0).floor().max(self.clip.0 as f32) as usize;
        let x1 = (((ax.max(bx) + half + 1.0).ceil() as usize) + 1).min(self.clip.2);
        let y0 = (ay.min(by) - half - 1.0).floor().max(self.clip.1 as f32) as usize;
        let y1 = (((ay.max(by) + half + 1.0).ceil() as usize) + 1).min(self.clip.3);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = capsule_dist(x as f32, y as f32, ax, ay, bx, by);
                let cover = (half - d + 0.5).clamp(0.0, 1.0);
                if cover > 0.0 {
                    self.blend(x, y, c, alpha * cover);
                }
            }
        }
    }

    /// Arc as in the original: the span walked in 20 line segments, with
    /// glowing endpoint dots.
    #[allow(clippy::too_many_arguments)]
    fn arc(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        start_deg: f32,
        span_deg: f32,
        thick: f32,
        c: Rgb,
        alpha: f32,
    ) {
        const SEGS: usize = 20;
        let mut prev: Option<(f32, f32)> = None;
        for k in 0..=SEGS {
            let a = (start_deg + span_deg * k as f32 / SEGS as f32).to_radians();
            let (sin, cos) = a.sin_cos();
            let p = (cx + cos * r * self.sx, cy + sin * r);
            if let Some(q) = prev {
                self.line(q.0, q.1, p.0, p.1, thick, c, alpha);
            }
            prev = Some(p);
        }
        for k in [0usize, SEGS] {
            let a = (start_deg + span_deg * k as f32 / SEGS as f32).to_radians();
            let (sin, cos) = a.sin_cos();
            self.dot(cx + cos * r * self.sx, cy + sin * r, 2.5, c, alpha);
        }
    }

    fn dot(&mut self, cx: f32, cy: f32, r: f32, c: Rgb, alpha: f32) {
        let (x0, x1) = self.clip_span_x(cx, r + 1.0);
        let (y0, y1) = self.clip_span_y(cy, r + 1.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = dist(x as f32, y as f32, cx, cy);
                let cover = (r - d + 0.5).clamp(0.0, 1.0);
                if cover > 0.0 {
                    self.blend(x, y, c, alpha * cover);
                }
            }
        }
    }

    /// A 5x7 glyph centred at (gx, gy), upright, with a 1 px dark outline
    /// (four offset passes, as the original text ring does).
    fn glyph_outlined(&mut self, ch: char, gx: f32, gy: f32, scale: usize, c: Rgb, alpha: f32, outline: Rgb) {
        let gw = (5 * scale) as f32;
        let gh = (7 * scale) as f32;
        let left = (gx - gw / 2.0) as i32;
        let top = (gy - gh / 2.0) as i32;
        let glyph = glyph5x7(ch);
        for pass in 0..5 {
            let (dx, dy, color, a) = match pass {
                0 => (-1i32, 0i32, outline, alpha * 0.9),
                1 => (1, 0, outline, alpha * 0.9),
                2 => (0, -1, outline, alpha * 0.9),
                3 => (0, 1, outline, alpha * 0.9),
                _ => (0, 0, c, alpha),
            };
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if bits & (0b10000 >> col) == 0 {
                        continue;
                    }
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let x = left + (col * scale + sx) as i32 + dx;
                            let y = top + (row * scale + sy) as i32 + dy;
                            if x >= 0 && y >= 0 {
                                self.blend(x as usize, y as usize, color, a);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn glyph5x7(c: char) -> [u8; 7] {
    match c {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        _ => [0; 7],
    }
}

// --------------------------------------------------------------- helpers

fn lerp3(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

fn dist(x: f32, y: f32, cx: f32, cy: f32) -> f32 {
    ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()
}

fn capsule_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    dist(px, py, ax + dx * t, ay + dy * t)
}

fn span(c: f32, r: f32, limit: usize) -> (usize, usize) {
    let lo = (c - r).floor().max(0.0) as usize;
    let hi = ((c + r).ceil() as usize + 1).min(limit);
    (lo, hi)
}

fn add_px_f(buf: &mut [f32], w: usize, h: usize, x: i32, y: i32, c: Rgb) {
    if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
        return;
    }
    let i = (y as usize * w + x as usize) * 3;
    buf[i] += c.0;
    buf[i + 1] += c.1;
    buf[i + 2] += c.2;
}

fn blend_px_f(buf: &mut [f32], w: usize, x: usize, y: usize, c: Rgb, a: f32) {
    let i = (y * w + x) * 3;
    buf[i] = buf[i] * (1.0 - a) + c.0 * a;
    buf[i + 1] = buf[i + 1] * (1.0 - a) + c.1 * a;
    buf[i + 2] = buf[i + 2] * (1.0 - a) + c.2 * a;
}

fn hash2(a: u32, b: u32) -> u32 {
    let mut x = a.wrapping_mul(0x85eb_ca6b) ^ b.wrapping_mul(0xc2b2_ae35);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

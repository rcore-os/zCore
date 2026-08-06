//! Early framebuffer console for x86_64 (UEFI GOP framebuffer).
//!
//! This is used to display boot logs on screen when no serial is available.
//! It is intentionally minimal: fixed 8x16 font, white text on black background.

#![cfg(feature = "graphic")]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use crate::KCONFIG;

// 8x8 font (public domain style) for ASCII 0x20..0x7F.
// Each byte is one row, MSB = leftmost pixel.
const FONT8X8: [[u8; 8]; 96] = include!("font8x8_basic.in");

static INITED: AtomicBool = AtomicBool::new(false);
static FB_BASE: AtomicUsize = AtomicUsize::new(0);
static FB_WIDTH: AtomicU32 = AtomicU32::new(0);
static FB_HEIGHT: AtomicU32 = AtomicU32::new(0);
static FB_STRIDE_PIXELS: AtomicU32 = AtomicU32::new(0);
static CUR_X: AtomicU32 = AtomicU32::new(0);
static CUR_Y: AtomicU32 = AtomicU32::new(0);
static CLEAR_ON_NEXT_TEXT_WRITE: AtomicBool = AtomicBool::new(false);
static ROT180: AtomicBool = AtomicBool::new(false);

const CHAR_W: u32 = 8;
const CHAR_H: u32 = 16;

fn try_init() -> bool {
    if INITED.load(Ordering::SeqCst) {
        return true;
    }
    let cfg = match KCONFIG.try_get() {
        Some(cfg) => cfg,
        None => return false,
    };
    if cfg.fb_addr == 0 || cfg.fb_size == 0 {
        return false;
    }

    let (w, h) = cfg.fb_mode.resolution();
    let stride = cfg.fb_mode.stride();
    FB_BASE.store(
        crate::mem::phys_to_virt(cfg.fb_addr as usize),
        Ordering::SeqCst,
    );
    FB_WIDTH.store(w as u32, Ordering::SeqCst);
    FB_HEIGHT.store(h as u32, Ordering::SeqCst);
    // Use the actual GOP stride. Real hardware often pads rows.
    FB_STRIDE_PIXELS.store(stride as u32, Ordering::SeqCst);
    if cfg.cmdline.contains("FB_ROT180=1")
        || cfg.cmdline.contains("FB_ROT180=true")
        || cfg.cmdline.contains("FB_ROT180=on")
        || cfg.cmdline.contains("FB_ROT180")
    {
        ROT180.store(true, Ordering::SeqCst);
    }

    // IMPORTANT: do NOT clear on init.
    // We want the boot progress bar to be continuous from the bootloader (rboot)
    // into the kernel. The kernel will request a clear later when the native
    // graphic console takes over.
    INITED.store(true, Ordering::SeqCst);
    true
}

fn clear_black() {
    let base = FB_BASE.load(Ordering::SeqCst);
    let w = FB_WIDTH.load(Ordering::SeqCst) as usize;
    let h = FB_HEIGHT.load(Ordering::SeqCst) as usize;
    let stride = FB_STRIDE_PIXELS.load(Ordering::SeqCst) as usize;
    if base == 0 || w == 0 || h == 0 {
        return;
    }
    let fb_ptr = base as *mut u32;
    let count = stride * h;
    #[allow(unsafe_code)]
    unsafe {
        for i in 0..count {
            core::ptr::write_volatile(fb_ptr.add(i), 0xFF00_0000);
        }
    }
}

fn put_pixel(x: u32, y: u32, argb: u32) {
    let base = FB_BASE.load(Ordering::SeqCst);
    let w = FB_WIDTH.load(Ordering::SeqCst);
    let h = FB_HEIGHT.load(Ordering::SeqCst);
    if base == 0 || x >= w || y >= h {
        return;
    }
    let (mut x, mut y) = (x, y);
    if ROT180.load(Ordering::SeqCst) {
        x = w - 1 - x;
        y = h - 1 - y;
    }
    let stride = FB_STRIDE_PIXELS.load(Ordering::SeqCst) as usize;
    let idx = (y as usize) * stride + (x as usize);
    #[allow(unsafe_code)]
    unsafe {
        core::ptr::write_volatile((base as *mut u32).add(idx), argb);
    }
}

fn fill_rect(x: u32, y: u32, w: u32, h: u32, argb: u32) {
    let sw = FB_WIDTH.load(Ordering::SeqCst);
    let sh = FB_HEIGHT.load(Ordering::SeqCst);
    if sw == 0 || sh == 0 {
        return;
    }
    let x1 = (x + w).min(sw);
    let y1 = (y + h).min(sh);
    for yy in y..y1 {
        for xx in x..x1 {
            put_pixel(xx, yy, argb);
        }
    }
}

fn stroke_rect(x: u32, y: u32, w: u32, h: u32, thickness: u32, argb: u32) {
    if w == 0 || h == 0 || thickness == 0 {
        return;
    }
    // Top / bottom
    fill_rect(x, y, w, thickness, argb);
    if h > thickness {
        fill_rect(x, y + h - thickness, w, thickness, argb);
    }
    // Left / right
    if h > thickness * 2 {
        fill_rect(x, y + thickness, thickness, h - thickness * 2, argb);
        if w > thickness {
            fill_rect(
                x + w - thickness,
                y + thickness,
                thickness,
                h - thickness * 2,
                argb,
            );
        }
    }
}

fn newline() {
    CUR_X.store(0, Ordering::SeqCst);
    CUR_Y.fetch_add(1, Ordering::SeqCst);
    // crude scroll: clear screen when full
    let max_rows = FB_HEIGHT.load(Ordering::SeqCst) / CHAR_H;
    if CUR_Y.load(Ordering::SeqCst) >= max_rows {
        clear_black();
        CUR_Y.store(0, Ordering::SeqCst);
    }
}

fn draw_char(c: u8) {
    if c == b'\n' {
        newline();
        return;
    }
    if c == b'\r' {
        CUR_X.store(0, Ordering::SeqCst);
        return;
    }
    let c = if (0x20..0x80).contains(&c) { c } else { b'?' };
    let glyph = &FONT8X8[(c - 0x20) as usize];

    let col = CUR_X.load(Ordering::SeqCst);
    let row = CUR_Y.load(Ordering::SeqCst);
    let x0 = col * CHAR_W;
    let y0 = row * CHAR_H;

    // 8x16: draw each font row twice vertically
    for (gy, bits) in glyph.iter().copied().enumerate() {
        for gx in 0..8 {
            let on = (bits & (1 << (7 - gx))) != 0;
            if on {
                let px = x0 + gx as u32;
                let py = y0 + (gy as u32) * 2;
                put_pixel(px, py, 0xFFFF_FFFF);
                put_pixel(px, py + 1, 0xFFFF_FFFF);
            }
        }
    }

    CUR_X.fetch_add(1, Ordering::SeqCst);
    let max_cols = FB_WIDTH.load(Ordering::SeqCst) / CHAR_W;
    if CUR_X.load(Ordering::SeqCst) >= max_cols {
        newline();
    }
}

fn draw_char_at(x0: u32, y0: u32, c: u8, fg: u32, bg: u32) {
    let c = if (0x20..0x80).contains(&c) { c } else { b'?' };
    let glyph = &FONT8X8[(c - 0x20) as usize];

    // 8x16: draw each font row twice vertically
    for (gy, bits) in glyph.iter().copied().enumerate() {
        for gx in 0..8 {
            let on = (bits & (1 << (7 - gx))) != 0;
            let color = if on { fg } else { bg };
            let px = x0 + gx as u32;
            let py = y0 + (gy as u32) * 2;
            put_pixel(px, py, color);
            put_pixel(px, py + 1, color);
        }
    }
}

pub fn write_str(s: &str) {
    if !try_init() {
        return;
    }
    if CLEAR_ON_NEXT_TEXT_WRITE.swap(false, Ordering::SeqCst) {
        clear_black();
        CUR_X.store(0, Ordering::SeqCst);
        CUR_Y.store(0, Ordering::SeqCst);
    }
    for &b in s.as_bytes() {
        draw_char(b);
    }
}

/// LAST-RESORT panic banner: draw `text` on a red band across the top of the
/// framebuffer, using ONLY atomics and raw pixel writes — no mutex, no RefCell,
/// no allocation. This is the output of record when a panic happens while some
/// CPU (possibly the panicking one) holds the console/serial locks: every other
/// path can be silently dropped or deadlock, this one cannot. Multi-line text
/// wraps; the band grows to fit.
pub fn panic_banner(text: &str) {
    if !try_init() {
        return;
    }
    let sw = FB_WIDTH.load(Ordering::SeqCst);
    if sw == 0 {
        return;
    }
    let cols = (sw / CHAR_W).max(1);
    // Pre-measure wrapped lines to size the band.
    let mut lines: u32 = 1;
    let mut col: u32 = 0;
    for &b in text.as_bytes() {
        if b == b'\n' || col >= cols {
            lines += 1;
            col = 0;
            if b == b'\n' {
                continue;
            }
        }
        col += 1;
    }
    let band_h = (lines + 1) * CHAR_H;
    const RED: u32 = 0xFFCC_0000;
    const WHITE: u32 = 0xFFFF_FFFF;
    fill_rect(0, 0, sw, band_h, RED);
    let (mut x, mut y) = (0u32, CHAR_H / 2);
    for &b in text.as_bytes() {
        if b == b'\n' || x >= cols {
            x = 0;
            y += CHAR_H;
            if b == b'\n' {
                continue;
            }
        }
        draw_char_at(x * CHAR_W, y, b, WHITE, RED);
        x += 1;
    }
}

/// Draw a centered boot progress bar (0..=100) on the early framebuffer.
///
/// White border + white fill, black background inside bar area.
pub fn draw_progress_bar(progress: u32) {
    if !try_init() {
        return;
    }
    let sw = FB_WIDTH.load(Ordering::SeqCst);
    let sh = FB_HEIGHT.load(Ordering::SeqCst);
    if sw == 0 || sh == 0 {
        return;
    }

    let progress = progress.min(100);
    if progress >= 100 {
        CLEAR_ON_NEXT_TEXT_WRITE.store(true, Ordering::SeqCst);
    }
    let bar_w: u32 = 400;
    let bar_h: u32 = 20;
    let x = sw.saturating_sub(bar_w) / 2;
    // Position below the centered logo (LOGO_HEIGHT=250, so bottom is sh/2 + 125).
    // Offset sh/2 + 160 puts the bar 35px below the logo.
    let y = (sh / 2).saturating_add(160);

    // Outer border (1px) with 2px margin like the reference.
    stroke_rect(
        x.saturating_sub(2),
        y.saturating_sub(2),
        bar_w + 4,
        bar_h + 4,
        1,
        0xFF00_0000, // Black border
    );
    // Inner fill.
    let fill_w = (bar_w * progress) / 100;
    if fill_w > 0 {
        fill_rect(x, y, fill_w, bar_h, 0xFF00_0000); // Black fill
    }
    if fill_w < bar_w {
        fill_rect(x + fill_w, y, bar_w - fill_w, bar_h, 0xFFFF_FFFF); // White remainder
    }

    // Fixed-width percentage text (4 chars).
    let p = progress.min(100);
    let mut buf = [b' '; 4];
    buf[3] = b'%';
    if p == 100 {
        buf[0] = b'1';
        buf[1] = b'0';
        buf[2] = b'0';
    } else if p >= 10 {
        buf[1] = b'0' + (p / 10) as u8;
        buf[2] = b'0' + (p % 10) as u8;
    } else {
        buf[2] = b'0' + p as u8;
    }
    let text_w: u32 = 4 * 8;
    let tx = x + (bar_w.saturating_sub(text_w)) / 2;
    let ty = y + bar_h + 15;
    for (i, ch) in buf.iter().copied().enumerate() {
        draw_char_at(tx + (i as u32) * 8, ty, ch, 0xFF00_0000, 0xFFFF_FFFF); // Black text on white
    }
}

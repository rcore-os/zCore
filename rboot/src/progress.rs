//! Boot progress bar drawing (UEFI GOP framebuffer).
//!
//! Minimal, no_std-friendly: draws a centered progress bar directly into GOP fb.

use core::sync::atomic::{AtomicBool, Ordering};
use uefi::proto::console::gop::{ModeInfo, PixelFormat};

// 8x8 font for ASCII 0x20..0x7F (same data format as kernel-hal).
const FONT8X8: [[u8; 8]; 96] = include!("font8x8_basic.in");

static ROT180: AtomicBool = AtomicBool::new(false);

/// Rotate all progress drawing by 180 degrees (useful on some real panels/firmware).
pub fn set_rot180(enable: bool) {
    ROT180.store(enable, Ordering::SeqCst);
}

fn pixel_white(fmt: PixelFormat) -> u32 {
    match fmt {
        // Our code writes low bytes as B,G,R so 0x00RRGGBB works for Bgr.
        PixelFormat::Bgr | PixelFormat::BltOnly => 0x00FF_FFFF,
        // For Rgb, low bytes are R,G,B, so use 0x00BBGGRR (white is same either way).
        PixelFormat::Rgb => 0x00FF_FFFF,
        _ => 0x00FF_FFFF,
    }
}

fn pixel_black(_fmt: PixelFormat) -> u32 {
    0x0000_0000
}

fn put_pixel(fb: *mut u32, stride: usize, sw: usize, sh: usize, x: usize, y: usize, pixel: u32) {
    let (mut x, mut y) = (x, y);
    if ROT180.load(Ordering::SeqCst) {
        x = sw.saturating_sub(1).saturating_sub(x);
        y = sh.saturating_sub(1).saturating_sub(y);
    }
    unsafe { core::ptr::write_volatile(fb.add(y * stride + x), pixel) };
}

fn draw_char_8x16(
    fb: *mut u32,
    stride: usize,
    sw: usize,
    sh: usize,
    x: usize,
    y: usize,
    c: u8,
    fg: u32,
    bg: u32,
) {
    let c = if (0x20..0x80).contains(&c) { c } else { b'?' };
    let glyph = &FONT8X8[(c - 0x20) as usize];
    for (gy, bits) in glyph.iter().copied().enumerate() {
        let py0 = y + gy * 2;
        if py0 >= sh {
            break;
        }
        for gx in 0..8 {
            let px = x + gx;
            if px >= sw {
                break;
            }
            let on = (bits & (1 << (7 - gx))) != 0;
            let color = if on { fg } else { bg };
            if py0 < sh {
                put_pixel(fb, stride, sw, sh, px, py0, color);
            }
            if py0 + 1 < sh {
                put_pixel(fb, stride, sw, sh, px, py0 + 1, color);
            }
        }
    }
}

fn draw_text_8x16(
    fb: *mut u32,
    stride: usize,
    sw: usize,
    sh: usize,
    x: usize,
    y: usize,
    text: &[u8],
    fg: u32,
    bg: u32,
) {
    let mut cx = x;
    for &ch in text {
        draw_char_8x16(fb, stride, sw, sh, cx, y, ch, fg, bg);
        cx = cx.saturating_add(8);
        if cx >= sw {
            break;
        }
    }
}

fn fill_rect(
    fb: *mut u32,
    stride: usize,
    sw: usize,
    sh: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    pixel: u32,
) {
    let x1 = (x + w).min(sw);
    let y1 = (y + h).min(sh);
    for yy in y..y1 {
        for xx in x..x1 {
            put_pixel(fb, stride, sw, sh, xx, yy, pixel);
        }
    }
}

fn stroke_rect(
    fb: *mut u32,
    stride: usize,
    sw: usize,
    sh: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    t: usize,
    pixel: u32,
) {
    if w == 0 || h == 0 || t == 0 {
        return;
    }
    fill_rect(fb, stride, sw, sh, x, y, w, t, pixel);
    if h > t {
        fill_rect(fb, stride, sw, sh, x, y + h - t, w, t, pixel);
    }
    if h > t * 2 {
        fill_rect(fb, stride, sw, sh, x, y + t, t, h - t * 2, pixel);
        if w > t {
            fill_rect(fb, stride, sw, sh, x + w - t, y + t, t, h - t * 2, pixel);
        }
    }
}

/// Draw a centered progress bar (0..=100).
///
/// `pixel` encoding matches existing `logo.rs`: 0x00RRGGBB.
pub fn bar(mode: ModeInfo, fb_addr: u64, progress: u32) {
    let fmt = mode.pixel_format();
    // BltOnly has no direct framebuffer access; skip.
    // Bitmask (used by NVIDIA and other vendors) and all other 32bpp formats
    // are written with white (0x00FF_FFFF) and black (0x0000_0000), which are
    // channel-layout-independent, so drawing proceeds regardless.
    if fmt == PixelFormat::BltOnly {
        return;
    }

    let (sw, sh) = mode.resolution();
    let sw = sw as usize;
    let sh = sh as usize;
    let stride = mode.stride() as usize;
    let fb = fb_addr as *mut u32;

    let progress = (progress.min(100)) as usize;
    let bar_w: usize = 400;
    let bar_h: usize = 20;
    let x = sw.saturating_sub(bar_w) / 2;
    // Position below the centered logo.
    // The logo is centered: its bottom is sh/2 + LOGO_HEIGHT/2.
    // We add 35px padding.
    let y = (sh / 2)
        .saturating_add(crate::logo::LOGO_HEIGHT / 2)
        .saturating_add(35);

    // Border (black) and inner content (black fill / white remainder).
    let white = pixel_white(fmt);
    let black = pixel_black(fmt);
    stroke_rect(
        fb,
        stride,
        sw,
        sh,
        x.saturating_sub(2),
        y.saturating_sub(2),
        bar_w + 4,
        bar_h + 4,
        1,
        black,
    );
    let fill_w = (bar_w * progress) / 100;
    if fill_w > 0 {
        fill_rect(fb, stride, sw, sh, x, y, fill_w, bar_h, black);
    }
    if fill_w < bar_w {
        fill_rect(
            fb,
            stride,
            sw,
            sh,
            x + fill_w,
            y,
            bar_w - fill_w,
            bar_h,
            white,
        );
    }

    // Fixed-width percentage text (4 chars: "100%" or "  7%"/" 42%").
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
    let text_w = 4 * 8;
    let tx = x + (bar_w.saturating_sub(text_w)) / 2;
    let ty = y + bar_h + 15;
    draw_text_8x16(fb, stride, sw, sh, tx, ty, &buf, black, white);
}

/// Draw the same bar using raw framebuffer parameters.
pub fn bar_raw(fb_addr: u64, stride: usize, sw: usize, sh: usize, progress: u32) {
    let fb = fb_addr as *mut u32;
    let progress = (progress.min(100)) as usize;
    let bar_w: usize = 400;
    let bar_h: usize = 20;
    let x = sw.saturating_sub(bar_w) / 2;
    // Same offset as bar()
    let y = (sh / 2)
        .saturating_add(crate::logo::LOGO_HEIGHT / 2)
        .saturating_add(35);
    let white: u32 = 0x00FF_FFFF;
    let black: u32 = 0x0000_0000;

    stroke_rect(
        fb,
        stride,
        sw,
        sh,
        x.saturating_sub(2),
        y.saturating_sub(2),
        bar_w + 4,
        bar_h + 4,
        1,
        black,
    );
    let fill_w = (bar_w * progress) / 100;
    if fill_w > 0 {
        fill_rect(fb, stride, sw, sh, x, y, fill_w, bar_h, black);
    }
    if fill_w < bar_w {
        fill_rect(
            fb,
            stride,
            sw,
            sh,
            x + fill_w,
            y,
            bar_w - fill_w,
            bar_h,
            white,
        );
    }

    // Percentage text (fixed width).
    let p = progress as u32;
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
    let text_w = 4 * 8;
    let tx = x + (bar_w.saturating_sub(text_w)) / 2;
    let ty = y + bar_h + 15;
    draw_text_8x16(fb, stride, sw, sh, tx, ty, &buf, black, white);
}

/// Draw a small fault marker block at top-left, encoding `tag` and `code` as pixels.
pub fn fault_block_raw(fb_addr: u64, stride: usize, sw: usize, sh: usize, tag: u32, code: u32) {
    let fb = fb_addr as *mut u32;
    let w = sw.min(64);
    let h = sh.min(32);
    let base_x = 8usize;
    let base_y = 8usize;
    let white: u32 = 0x00FF_FFFF;
    let red: u32 = 0x0000_00FF; // best-effort visible
    let black: u32 = 0x0000_0000;

    // Background.
    fill_rect(fb, stride, sw, sh, base_x, base_y, w - 16, h - 16, black);
    // Border.
    stroke_rect(fb, stride, sw, sh, base_x, base_y, w - 16, h - 16, 1, white);
    // A red stripe.
    fill_rect(fb, stride, sw, sh, base_x + 2, base_y + 2, 8, h - 20, red);

    // Encode tag/code in a few pixels.
    let t = tag ^ (code.rotate_left(7));
    for i in 0..16usize {
        let bit = (t >> i) & 1;
        let px = base_x + 14 + i;
        let py = base_y + 4;
        if px < sw && py < sh {
            put_pixel(
                fb,
                stride,
                sw,
                sh,
                px,
                py,
                if bit == 1 { white } else { black },
            );
        }
    }
}

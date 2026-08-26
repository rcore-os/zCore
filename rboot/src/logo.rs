//! Boot logo drawing (UEFI GOP framebuffer).
//!
//! The logo is expected to be an 800x250 raw BGRA image (32bpp), same as the
//! userspace `display_service` asset.

use uefi::proto::console::gop::{ModeInfo, PixelFormat};

pub const LOGO_WIDTH: usize = 800;
pub const LOGO_HEIGHT: usize = 250;

// NOTE: absolute path per request. Consider vendoring into `rboot/src/` later.
const LOGO_DATA: &[u8] = include_bytes!("logo.raw");

pub fn draw_centered(mode: ModeInfo, fb_addr: u64) {
    let (sw, sh) = mode.resolution();
    let sw = sw as usize;
    let sh = sh as usize;
    let stride = mode.stride() as usize;

    // Only handle 32bpp formats we can map to.
    let fmt = mode.pixel_format();
    if fmt != PixelFormat::Bgr && fmt != PixelFormat::BltOnly && fmt != PixelFormat::Rgb {
        // We only expect Bgr on QEMU/UEFI in this setup.
        // If it's something else, just skip.
        return;
    }

    let start_x = sw.saturating_sub(LOGO_WIDTH) / 2;
    let start_y = sh.saturating_sub(LOGO_HEIGHT) / 2;

    let fb_ptr = fb_addr as *mut u32;

    // Clear to white (match the asset background expectation).
    unsafe {
        for y in 0..sh {
            let row = y * stride;
            for x in 0..sw {
                core::ptr::write_volatile(fb_ptr.add(row + x), 0x00FF_FFFF);
            }
        }
    }

    // Draw logo. LOGO_DATA is BGRA (b,g,r,a). GOP is typically BGRX/BGRA.
    unsafe {
        for y in 0..LOGO_HEIGHT {
            let sy = start_y + y;
            if sy >= sh {
                break;
            }
            for x in 0..LOGO_WIDTH {
                let sx = start_x + x;
                if sx >= sw {
                    break;
                }
                let idx = (y * LOGO_WIDTH + x) * 4;
                if idx + 3 >= LOGO_DATA.len() {
                    return;
                }
                let b = LOGO_DATA[idx] as u32;
                let g = LOGO_DATA[idx + 1] as u32;
                let r = LOGO_DATA[idx + 2] as u32;
                let a = LOGO_DATA[idx + 3] as u32;
                if a == 0 {
                    continue;
                }
                // Write as 0x00RRGGBB; UEFI GOP Bgr generally maps this as B,G,R in low bytes.
                let pixel = (r << 16) | (g << 8) | b;
                core::ptr::write_volatile(fb_ptr.add(sy * stride + sx), pixel);
            }
        }
    }
}

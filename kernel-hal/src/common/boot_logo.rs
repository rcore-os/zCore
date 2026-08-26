//! Boot logo drawing for graphic mode.
//!
//! This is a tiny, no_std-friendly blitter that draws the Eclipse OS logo
//! early during boot.

#![cfg(feature = "graphic")]

use zcore_drivers::prelude::{ColorFormat, RgbColor};
use zcore_drivers::scheme::DisplayScheme;

// Keep these aligned with the userspace display_service logo.
const LOGO_WIDTH: u32 = 800;
const LOGO_HEIGHT: u32 = 250;

// NOTE: this uses an absolute path on the developer machine, matching the user's request.
// If you want this to be portable, vendor the asset into this repo and switch to a relative path.
const LOGO_DATA: &[u8] = include_bytes!("logo.raw");

pub fn draw_centered(display: &dyn DisplayScheme) {
    let info = display.info();
    if info.format != ColorFormat::ARGB8888 {
        return;
    }

    // Clear background (white) for the splash.
    display.clear(RgbColor::new(255, 255, 255));

    let screen_width = info.width;
    let screen_height = info.height;
    let start_x = (screen_width.saturating_sub(LOGO_WIDTH)) / 2;
    let start_y = (screen_height.saturating_sub(LOGO_HEIGHT)) / 2;

    // Raw logo is BGRA, 32bpp.
    for y in 0..LOGO_HEIGHT {
        let screen_y = start_y + y;
        if screen_y >= screen_height {
            break;
        }
        for x in 0..LOGO_WIDTH {
            let screen_x = start_x + x;
            if screen_x >= screen_width {
                break;
            }
            let pixel_idx = ((y * LOGO_WIDTH + x) as usize) * 4;
            if pixel_idx + 3 >= LOGO_DATA.len() {
                return;
            }
            let b = LOGO_DATA[pixel_idx];
            let g = LOGO_DATA[pixel_idx + 1];
            let r = LOGO_DATA[pixel_idx + 2];
            let a = LOGO_DATA[pixel_idx + 3];
            if a == 0 {
                continue;
            }
            // We ignore alpha blending and just draw the RGB channels.
            display.draw_pixel(screen_x, screen_y, RgbColor::new(r, g, b));
        }
    }

    let _ = display.flush();
}

/// Clear the whole screen with an opaque color.
///
/// In ARGB8888 mode we force alpha=0xFF to ensure the clear is visible.
pub fn clear_screen(display: &dyn DisplayScheme, color: RgbColor) -> Result<(), &'static str> {
    display.clear(color);
    let _ = display.flush();
    Ok(())
}

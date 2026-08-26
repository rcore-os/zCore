use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::convert::Infallible;
use core::ops::{Deref, DerefMut};

use rcore_console::embedded_graphics::prelude::RgbColor as _;
use rcore_console::{
    Cell, Console, DrawTarget, Flags, OriginDimensions, Pixel, Rgb888, Size, TextBuffer,
    TextOnGraphic,
};

use super::shadow_fb::ShadowFramebuffer;
use crate::scheme::display::DisplayScheme;

/// Height in pixels of one text row (matches `rcore_console`'s `FONT_9X18`).
const CHAR_HEIGHT: usize = 18;
/// Width in pixels of one character cell (matches `rcore_console`'s `FONT_9X18`).
const CHAR_WIDTH: usize = 9;

/// Convert an `rcore_console` glyph color to a packed `0x00RRGGBB` value, matching
/// the byte layout previously written straight to the ARGB8888 framebuffer.
#[inline]
fn rgb888_to_argb(color: Rgb888) -> u32 {
    ((color.r() as u32) << 16) | ((color.g() as u32) << 8) | (color.b() as u32)
}

/// A `DrawTarget` that renders into a CPU-side [`ShadowFramebuffer`] instead of
/// writing pixels straight to GPU memory. Glyph rendering therefore touches only
/// cached RAM; the dirty region is later pushed to the device in bulk.
pub struct ShadowDraw {
    shadow: Arc<ShadowFramebuffer>,
    width: u32,
    height: u32,
}

impl DrawTarget for ShadowDraw {
    type Color = Rgb888;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        self.shadow.put_pixels(pixels.into_iter().filter_map(|p| {
            let (x, y) = (p.0.x, p.0.y);
            if x < 0 || y < 0 {
                return None;
            }
            Some((x as usize, y as usize, rgb888_to_argb(p.1)))
        }));
        Ok(())
    }
}

impl OriginDimensions for ShadowDraw {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

pub struct LinearScrollbackBuffer {
    buf: Vec<Vec<Cell>>,
    history: VecDeque<Vec<Cell>>,
    scrollback_offset: Option<usize>,
    inner: TextOnGraphic<ShadowDraw>,
    shadow: Arc<ShadowFramebuffer>,
    display: Arc<dyn DisplayScheme>,
    /// Best-effort text cursor position (cell coords) for the block cursor.
    /// Tracks where the next character will be drawn.
    cursor_row: usize,
    cursor_col: usize,
}

impl LinearScrollbackBuffer {
    pub fn new(display: Arc<dyn DisplayScheme>) -> Self {
        let info = display.info();
        let shadow = ShadowFramebuffer::new(info.width as usize, info.height as usize);
        let draw = ShadowDraw {
            shadow: shadow.clone(),
            width: info.width,
            height: info.height,
        };
        let inner = TextOnGraphic::new(draw, info.width, info.height);
        let width = inner.width();
        let height = inner.height();
        Self {
            buf: vec![vec![Cell::default(); width]; height],
            history: VecDeque::new(),
            scrollback_offset: None,
            inner,
            shadow,
            display,
            cursor_row: 0,
            cursor_col: 0,
        }
    }

    /// Push the dirty region of the shadow buffer to the real display, drawing
    /// the text cursor when `visible`.
    ///
    /// Called once per batch of writes (per `write_str` / scroll) so a whole
    /// line of output becomes a single bulk transfer to the GPU. The cursor is
    /// hidden while viewing scrollback history.
    pub fn present(&self, visible: bool) {
        let cursor = if visible && self.scrollback_offset.is_none() {
            Some((self.cursor_col, self.cursor_row))
        } else {
            None
        };
        self.shadow
            .present_with_cursor(&*self.display, cursor, CHAR_WIDTH, CHAR_HEIGHT);
    }

    /// Set the text cursor position (cell coords) used to draw the block cursor.
    ///
    /// Driven from the owning [`Console`]'s authoritative cursor so the block
    /// cursor follows `goto`/`move_*` escape sequences, not just the last cell
    /// written (which is where full-screen editors like nano leave it).
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = row;
        self.cursor_col = col;
    }

    /// Repaint the whole screen from the backing buffer into the shadow.
    ///
    /// Used when this VT becomes the active one (a different VT or a graphics
    /// client may have left arbitrary pixels on screen), so the shadow is first
    /// cleared to black and then every cell is redrawn.
    pub fn repaint_all(&mut self) {
        self.shadow.clear(0x0000_0000);
        self.redraw();
    }

    pub fn scroll_history(&mut self, direction: i32) {
        let height = self.height();
        let scroll_amount = (height as i32 - 2).max(1);
        let delta = direction * scroll_amount;
        let history_len = self.history.len();

        if delta > 0 {
            // Scroll up (back in history)
            let current_offset = self.scrollback_offset.unwrap_or(0);
            let new_offset = (current_offset + delta as usize).min(history_len);
            if new_offset > 0 {
                self.scrollback_offset = Some(new_offset);
            }
        } else if delta < 0 {
            // Scroll down (forward in history)
            if let Some(current_offset) = self.scrollback_offset {
                let steps = (-delta) as usize;
                if current_offset <= steps {
                    self.scrollback_offset = None;
                } else {
                    self.scrollback_offset = Some(current_offset - steps);
                }
            }
        }

        self.redraw();
    }

    pub fn redraw(&mut self) {
        let height = self.height();
        let width = self.width();

        if let Some(offset) = self.scrollback_offset {
            let history_len = self.history.len();
            for r in 0..height {
                let index = (history_len as isize)
                    - (offset as isize)
                    - ((height as isize) - 1 - (r as isize));
                if index < 0 {
                    let bg_cell = Cell::default();
                    for col in 0..width {
                        self.inner.write(r, col, bg_cell);
                    }
                } else if index < history_len as isize {
                    let line = &self.history[index as usize];
                    for col in 0..width {
                        let cell = line[col];
                        self.inner.write(r, col, cell);
                    }
                } else {
                    let active_row = (index - history_len as isize) as usize;
                    if active_row < height {
                        let line = &self.buf[active_row];
                        for col in 0..width {
                            let cell = line[col];
                            self.inner.write(r, col, cell);
                        }
                    } else {
                        let bg_cell = Cell::default();
                        for col in 0..width {
                            self.inner.write(r, col, bg_cell);
                        }
                    }
                }
            }
        } else {
            for r in 0..height {
                let line = &self.buf[r];
                for col in 0..width {
                    let cell = line[col];
                    self.inner.write(r, col, cell);
                }
            }
        }
    }
}

impl TextBuffer for LinearScrollbackBuffer {
    #[inline]
    fn width(&self) -> usize {
        self.inner.width()
    }

    #[inline]
    fn height(&self) -> usize {
        self.inner.height()
    }

    #[inline]
    fn read(&self, row: usize, col: usize) -> Cell {
        self.buf[row][col]
    }

    #[inline]
    fn write(&mut self, row: usize, col: usize, cell: Cell) {
        let height = self.height();
        let width = self.width();
        if row >= height || col >= width {
            return;
        }
        // Skip the glyph rasterization when the cell already holds this exact
        // value: full-screen TUI repaints (htop, vim) rewrite >90 % identical
        // cells per refresh, and each `inner.write` is a per-pixel
        // embedded-graphics draw. `buf` always mirrors the last value pushed
        // to the pixels (including transient cursor-inversion writes), so
        // equality here guarantees the pixels are already correct. Cursor
        // tracking below still runs — position advances even over unchanged
        // cells.
        let unchanged = self.buf[row][col] == cell;
        self.buf[row][col] = cell;

        if self.scrollback_offset.is_none() {
            if !unchanged {
                self.inner.write(row, col, cell);
            }
            // Track the cursor as the position just after the written cell.
            self.cursor_row = row;
            self.cursor_col = col + 1;
            if self.cursor_col >= width {
                self.cursor_col = 0;
                self.cursor_row = (self.cursor_row + 1).min(height.saturating_sub(1));
            }
        }
    }

    fn new_line(&mut self, cell: Cell) {
        let height = self.height();
        let width = self.width();
        if height == 0 {
            return;
        }

        // 1+2. Rotate the rows instead of cloning each one: `rotate_left`
        // moves row 0 (the row scrolling into history) to the end as pure
        // pointer swaps, where the per-row `clone()` loop this replaces did
        // ~`height` heap alloc + memcpy + free pairs per newline. The old top
        // row's content goes to history; the row allocation recycled from a
        // retired history line (once scrollback is at capacity) becomes the
        // fresh bottom row, so a steady scroll allocates nothing at all.
        let bg_cell = Cell {
            c: ' ',
            bg: cell.bg,
            fg: cell.fg,
            flags: Flags::empty(),
        };
        self.buf.rotate_left(1);
        let mut new_bottom = if self.history.len() >= 1000 {
            self.history.pop_front().unwrap_or_default()
        } else {
            Vec::with_capacity(width)
        };
        new_bottom.clear();
        new_bottom.resize(width, bg_cell);
        let old_top = core::mem::replace(&mut self.buf[height - 1], new_bottom);
        self.history.push_back(old_top);

        // 3. Handle scrollback offset and scrolling
        if let Some(offset) = self.scrollback_offset {
            let max_offset = self.history.len();
            self.scrollback_offset = Some((offset + 1).min(max_offset));
            self.redraw();
        } else {
            // Scroll the shadow buffer up by one text row, entirely in cached
            // RAM — no read-back from GPU memory.
            let width_px = self.shadow.width();
            let text_h = height * CHAR_HEIGHT;
            if text_h > CHAR_HEIGHT {
                self.shadow
                    .copy_rect(0, CHAR_HEIGHT, 0, 0, width_px, text_h - CHAR_HEIGHT);
            }
            let bg_argb = rgb888_to_argb(cell.bg.to_rgb());
            self.shadow.fill_rect(
                0,
                (height - 1) * CHAR_HEIGHT,
                width_px,
                CHAR_HEIGHT,
                bg_argb,
            );
        }
        // After a scroll the next character lands at the bottom-left.
        self.cursor_row = height - 1;
        self.cursor_col = 0;
    }

    fn clear(&mut self, cell: Cell) {
        let width = self.width();
        let height = self.height();
        let bg_cell = Cell {
            c: ' ',
            bg: cell.bg,
            fg: cell.fg,
            flags: Flags::empty(),
        };
        self.buf = vec![vec![bg_cell; width]; height];
        self.history.clear();
        self.scrollback_offset = None;

        let bg_argb = rgb888_to_argb(cell.bg.to_rgb());
        self.shadow.clear(bg_argb);
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    /// Scroll a sub-region up by `n` lines, moving the surviving pixel band in
    /// one bulk `copy_rect` (cached RAM) instead of re-rendering every glyph —
    /// the fast path for full-screen TUIs (irssi/htop) that scroll a window.
    fn scroll_region_up(&mut self, top: usize, bottom: usize, n: usize, blank: Cell) {
        let height = self.height();
        let width = self.width();
        if top > bottom || bottom >= height || n == 0 {
            return;
        }
        let n = n.min(bottom - top + 1);
        let blank_cell = Cell {
            c: ' ',
            bg: blank.bg,
            fg: blank.fg,
            flags: Flags::empty(),
        };
        // Shift the backing cells up within the region.
        for r in top..=bottom {
            self.buf[r] = if r + n <= bottom {
                self.buf[r + n].clone()
            } else {
                vec![blank_cell; width]
            };
        }
        if self.scrollback_offset.is_some() {
            self.redraw();
            return;
        }
        // Move the surviving pixels up, then clear the vacated band.
        let width_px = self.shadow.width();
        let survivors = (bottom - top + 1) - n;
        if survivors > 0 {
            self.shadow.copy_rect(
                0,
                (top + n) * CHAR_HEIGHT,
                0,
                top * CHAR_HEIGHT,
                width_px,
                survivors * CHAR_HEIGHT,
            );
        }
        let bg_argb = rgb888_to_argb(blank.bg.to_rgb());
        self.shadow.fill_rect(
            0,
            (bottom + 1 - n) * CHAR_HEIGHT,
            width_px,
            n * CHAR_HEIGHT,
            bg_argb,
        );
    }

    /// Scroll a sub-region down by `n` lines (bulk pixel copy + clear the top).
    fn scroll_region_down(&mut self, top: usize, bottom: usize, n: usize, blank: Cell) {
        let height = self.height();
        let width = self.width();
        if top > bottom || bottom >= height || n == 0 {
            return;
        }
        let n = n.min(bottom - top + 1);
        let blank_cell = Cell {
            c: ' ',
            bg: blank.bg,
            fg: blank.fg,
            flags: Flags::empty(),
        };
        // Shift the backing cells down within the region (high rows first).
        for r in (top..=bottom).rev() {
            self.buf[r] = if r >= top + n {
                self.buf[r - n].clone()
            } else {
                vec![blank_cell; width]
            };
        }
        if self.scrollback_offset.is_some() {
            self.redraw();
            return;
        }
        let width_px = self.shadow.width();
        let survivors = (bottom - top + 1) - n;
        if survivors > 0 {
            self.shadow.copy_rect(
                0,
                top * CHAR_HEIGHT,
                0,
                (top + n) * CHAR_HEIGHT,
                width_px,
                survivors * CHAR_HEIGHT,
            );
        }
        let bg_argb = rgb888_to_argb(blank.bg.to_rgb());
        self.shadow
            .fill_rect(0, top * CHAR_HEIGHT, width_px, n * CHAR_HEIGHT, bg_argb);
    }
}

pub struct GraphicConsole {
    inner: Console<LinearScrollbackBuffer>,
}

impl GraphicConsole {
    pub fn new(display: Arc<dyn DisplayScheme>) -> Self {
        Self {
            inner: Console::on_text_buffer(LinearScrollbackBuffer::new(display)),
        }
    }

    /// Flush all pending console output to the display, showing the cursor.
    ///
    /// Drawing accumulates in the shadow buffer; this pushes the dirty region to
    /// the GPU in one bulk transfer. Call it after a batch of writes.
    pub fn present(&mut self) {
        let (row, col) = self.inner.cursor();
        // Honor DECTCEM (`?25`): a hidden cursor (full-screen TUIs while
        // repainting) is never drawn.
        let visible = self.inner.cursor_visible();
        let buf = self.inner.buf_mut();
        buf.set_cursor(row, col);
        buf.present(visible);
    }

    /// Redraw only the blinking cursor with the given visibility.
    ///
    /// Called from the timer tick (~2 Hz) so the cursor blinks while idle,
    /// without touching the text content. A cursor the application has hidden
    /// (`?25l`) must never blink back into view.
    pub fn set_cursor_blink(&mut self, visible: bool) {
        let (row, col) = self.inner.cursor();
        let visible = visible && self.inner.cursor_visible();
        let buf = self.inner.buf_mut();
        buf.set_cursor(row, col);
        buf.present(visible);
    }

    /// Repaint the entire screen from the backing buffer (e.g. on VT switch).
    pub fn repaint(&mut self) {
        self.inner.buf_mut().repaint_all();
        self.present();
    }
}

impl Deref for GraphicConsole {
    type Target = Console<LinearScrollbackBuffer>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for GraphicConsole {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

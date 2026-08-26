use crate::cell::{Cell, Flags};
use crate::text_buffer::TextBuffer;
use embedded_graphics::{
    mono_font::{
        iso_8859_1::{FONT_9X18 as FONT, FONT_9X18_BOLD as FONT_BOLD},
        MonoTextStyleBuilder,
    },
    pixelcolor::{Rgb888, RgbColor},
    prelude::{DrawTarget, Drawable, Point, Size},
    primitives::Rectangle,
    text::{Baseline, Text, TextStyle},
};

const CHAR_SIZE: Size = FONT.character_size;

/// Linearly blend `fg` toward `bg` (`num`/`den`), for the shade blocks (░▒▓).
fn blend(fg: Rgb888, bg: Rgb888, num: u32, den: u32) -> Rgb888 {
    let m = |a: u8, b: u8| (((a as u32) * num + (b as u32) * (den - num)) / den) as u8;
    Rgb888::new(m(fg.r(), bg.r()), m(fg.g(), bg.g()), m(fg.b(), bg.b()))
}

/// A [`TextBuffer`] on top of a frame buffer
///
/// The internal use [`embedded_graphics`] crate to render fonts to pixels.
///
/// The underlying frame buffer needs to implement `DrawTarget<Color = Rgb888>` trait
/// to draw pixels in RGB format.
pub struct TextOnGraphic<D>
where
    D: DrawTarget,
{
    width: u32,
    height: u32,
    graphic: D,
}

impl<D> TextOnGraphic<D>
where
    D: DrawTarget,
{
    /// Create a new text buffer on graphic.
    pub fn new(graphic: D, width: u32, height: u32) -> Self {
        TextOnGraphic {
            width,
            height,
            graphic,
        }
    }
}

impl<D> TextOnGraphic<D>
where
    D: DrawTarget<Color = Rgb888>,
{
    /// Fill a solid rectangle (in pixels). Errors from the backend are ignored,
    /// matching the rest of the renderer.
    #[inline]
    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgb888) {
        if w <= 0 || h <= 0 {
            return;
        }
        let _ = self.graphic.fill_solid(
            &Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32)),
            color,
        );
    }

    /// Draw box-drawing (U+2500..U+257F) and block-element (U+2580..U+2593)
    /// glyphs procedurally as rectangles. The bundled ISO-8859-1 font has no
    /// glyphs for these, so TUIs (htop, irssi, mc, dialog) would otherwise show
    /// blanks where borders and meters should be. Returns `true` if `c` was
    /// handled here (and the font path should be skipped).
    fn draw_special(&mut self, c: char, x: i32, y: i32, fg: Rgb888, bg: Rgb888) -> bool {
        let cw = CHAR_SIZE.width as i32;
        let ch = CHAR_SIZE.height as i32;
        let cx = x + cw / 2;
        let cy = y + ch / 2;

        // Box-drawing lines as (left, right, top, bottom) half-segments. Heavy,
        // double and rounded variants are approximated by the light geometry.
        let line = match c {
            '\u{2500}' | '\u{2501}' | '\u{2550}' => Some((true, true, false, false)),
            '\u{2502}' | '\u{2503}' | '\u{2551}' => Some((false, false, true, true)),
            '\u{250C}'..='\u{250F}' | '\u{2552}'..='\u{2554}' | '\u{256D}' => {
                Some((false, true, false, true))
            }
            '\u{2510}'..='\u{2513}' | '\u{2555}'..='\u{2557}' | '\u{256E}' => {
                Some((true, false, false, true))
            }
            '\u{2514}'..='\u{2517}' | '\u{2558}'..='\u{255A}' | '\u{2570}' => {
                Some((false, true, true, false))
            }
            '\u{2518}'..='\u{251B}' | '\u{255B}'..='\u{255D}' | '\u{256F}' => {
                Some((true, false, true, false))
            }
            '\u{251C}'..='\u{2523}' | '\u{255E}'..='\u{2560}' => Some((false, true, true, true)),
            '\u{2524}'..='\u{252B}' | '\u{2561}'..='\u{2563}' => Some((true, false, true, true)),
            '\u{252C}'..='\u{2533}' | '\u{2564}'..='\u{2566}' => Some((true, true, false, true)),
            '\u{2534}'..='\u{253B}' | '\u{2567}'..='\u{2569}' => Some((true, true, true, false)),
            '\u{253C}'..='\u{254B}' | '\u{256A}'..='\u{256C}' => Some((true, true, true, true)),
            _ => None,
        };
        if let Some((l, r, t, b)) = line {
            self.fill(x, y, cw, ch, bg);
            if l && r {
                self.fill(x, cy, cw, 1, fg);
            } else if l {
                self.fill(x, cy, cw / 2 + 1, 1, fg);
            } else if r {
                self.fill(cx, cy, cw - cw / 2, 1, fg);
            }
            if t && b {
                self.fill(cx, y, 1, ch, fg);
            } else if t {
                self.fill(cx, y, 1, ch / 2 + 1, fg);
            } else if b {
                self.fill(cx, cy, 1, ch - ch / 2, fg);
            }
            return true;
        }

        // Block elements.
        match c {
            '\u{2588}' => self.fill(x, y, cw, ch, fg), // full block
            '\u{2580}' => {
                self.fill(x, y, cw, ch, bg);
                self.fill(x, y, cw, ch / 2, fg); // upper half
            }
            '\u{2584}' => {
                self.fill(x, y, cw, ch, bg);
                self.fill(x, y + ch / 2, cw, ch - ch / 2, fg); // lower half
            }
            '\u{258C}' => {
                self.fill(x, y, cw, ch, bg);
                self.fill(x, y, cw / 2, ch, fg); // left half
            }
            '\u{2590}' => {
                self.fill(x, y, cw, ch, bg);
                self.fill(x + cw / 2, y, cw - cw / 2, ch, fg); // right half
            }
            '\u{2581}'..='\u{2587}' => {
                // lower 1/8..7/8 blocks (▁..▇)
                let n = c as i32 - 0x2580;
                let hpx = ch * n / 8;
                self.fill(x, y, cw, ch, bg);
                self.fill(x, y + ch - hpx, cw, hpx, fg);
            }
            '\u{2589}'..='\u{258F}' => {
                // left 7/8..1/8 blocks (▉..▏)
                let n = 8 - (c as i32 - 0x2588);
                let wpx = cw * n / 8;
                self.fill(x, y, cw, ch, bg);
                self.fill(x, y, wpx, ch, fg);
            }
            '\u{2591}' => self.fill(x, y, cw, ch, blend(fg, bg, 1, 4)), // light shade
            '\u{2592}' => self.fill(x, y, cw, ch, blend(fg, bg, 2, 4)), // medium shade
            '\u{2593}' => self.fill(x, y, cw, ch, blend(fg, bg, 3, 4)), // dark shade
            _ => return false,
        }
        true
    }
}

impl<D> TextBuffer for TextOnGraphic<D>
where
    D: DrawTarget<Color = Rgb888>,
{
    #[inline]
    fn width(&self) -> usize {
        (self.width / CHAR_SIZE.width) as usize
    }

    #[inline]
    fn height(&self) -> usize {
        (self.height / CHAR_SIZE.height) as usize
    }

    fn read(&self, _row: usize, _col: usize) -> Cell {
        unimplemented!("reading char from graphic is unsupported")
    }

    #[inline]
    fn write(&mut self, row: usize, col: usize, cell: Cell) {
        if row >= self.height() || col >= self.width() {
            return;
        }
        let (fg, bg) = if cell.flags.contains(Flags::INVERSE) {
            (cell.bg, cell.fg)
        } else {
            (cell.fg, cell.bg)
        };
        // Box-drawing / block-element glyphs are not in the bundled font; draw
        // them procedurally so TUI borders and meters render.
        let x = col as i32 * CHAR_SIZE.width as i32;
        let y = row as i32 * CHAR_SIZE.height as i32;
        if self.draw_special(cell.c, x, y, fg.to_rgb(), bg.to_rgb()) {
            return;
        }
        let mut utf8_buf = [0u8; 8];
        let s = cell.c.encode_utf8(&mut utf8_buf);
        let mut style = MonoTextStyleBuilder::new()
            .text_color(fg.to_rgb())
            .background_color(bg.to_rgb());
        if cell.flags.contains(Flags::BOLD) {
            style = style.font(&FONT_BOLD);
        } else {
            style = style.font(&FONT);
        }
        if cell.flags.contains(Flags::STRIKEOUT) {
            style = style.strikethrough();
        }
        if cell.flags.contains(Flags::UNDERLINE) {
            style = style.underline();
        }
        let text = Text::with_text_style(
            s,
            Point::new(
                col as i32 * CHAR_SIZE.width as i32,
                row as i32 * CHAR_SIZE.height as i32,
            ),
            style.build(),
            TextStyle::with_baseline(Baseline::Top),
        );
        text.draw(&mut self.graphic).ok();
    }
}

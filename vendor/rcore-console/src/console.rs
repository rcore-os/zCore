use crate::ansi::{Attr, ClearMode, Handler, LineClearMode, Mode, Performer};
use crate::cell::{Cell, Flags};
use crate::color::Rgb888;
use crate::graphic::TextOnGraphic;
use crate::text_buffer::TextBuffer;
use crate::text_buffer_cache::TextBufferCache;
use alloc::collections::VecDeque;
use core::cmp::min;
use core::fmt;

use embedded_graphics::prelude::{DrawTarget, OriginDimensions};
use vte::Parser;

/// Console
///
/// Input string with control sequence, output to a [`TextBuffer`].
pub struct Console<T: TextBuffer> {
    /// ANSI escape sequence parser
    parser: Parser,
    /// Inner state
    inner: ConsoleInner<T>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Cursor {
    row: usize,
    col: usize,
}

/// Saved main screen contents while the alternate screen buffer is active.
struct AltScreen {
    cells: alloc::vec::Vec<alloc::vec::Vec<Cell>>,
    cursor: Cursor,
}

struct ConsoleInner<T: TextBuffer> {
    /// cursor
    cursor: Cursor,
    /// Saved cursor
    saved_cursor: Cursor,
    /// current attribute template
    temp: Cell,
    /// character buffer
    buf: T,
    /// auto wrap
    auto_wrap: bool,
    /// Reported data for CSI Device Status Report
    report: VecDeque<u8>,
    /// Scroll region `(top, bottom)` inclusive, 0-indexed. `None` = whole
    /// screen, in which case scrolling uses the buffer's fast `new_line` path
    /// (which also feeds the scrollback history).
    scroll_region: Option<(usize, usize)>,
    /// Cursor visibility (DECTCEM, `?25`). Full-screen apps hide it while
    /// redrawing; the renderer consults this so a hidden cursor never blinks.
    cursor_visible: bool,
    /// Saved main screen while the alternate screen buffer is active (`?1049`,
    /// `?1047`, `?47`). `Some` means we are currently on the alternate screen.
    alt_saved: Option<AltScreen>,
    /// G0 charset is the DEC Special Graphics set (`ESC ( 0`). While active the
    /// VT100 line-drawing letters (`q`,`x`,`l`,…) map to Unicode box-drawing
    /// characters, the fallback for apps that draw borders without UTF-8.
    g0_dec: bool,
}

/// Map a VT100 DEC Special Graphics byte to its Unicode box-drawing/block
/// character (only the line-drawing and block subset that matters for TUIs).
fn dec_special_char(c: char) -> Option<char> {
    Some(match c {
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'q' | 'o' | 'p' | 'r' | 's' => '─',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'a' => '▒',
        '0' => '█',
        _ => return None,
    })
}

/// Console on top of a frame buffer
pub type ConsoleOnGraphic<D> = Console<TextBufferCache<TextOnGraphic<D>>>;

impl<D: DrawTarget<Color = Rgb888> + OriginDimensions> Console<TextBufferCache<TextOnGraphic<D>>> {
    /// Create a console on top of a frame buffer
    pub fn on_frame_buffer(buffer: D) -> Self {
        let size = buffer.size();
        Self::on_cached_text_buffer(TextOnGraphic::new(buffer, size.width, size.height))
    }
}

impl<T: TextBuffer> Console<TextBufferCache<T>> {
    /// Create a console on top of a [`TextBuffer`] with a cache layer
    pub fn on_cached_text_buffer(buffer: T) -> Self {
        Self::on_text_buffer(TextBufferCache::new(buffer))
    }
}

impl<T: TextBuffer> Console<T> {
    /// Create a console on top of a [`TextBuffer`]
    pub fn on_text_buffer(buffer: T) -> Self {
        Console {
            parser: Parser::new(),
            inner: ConsoleInner {
                cursor: Cursor::default(),
                saved_cursor: Cursor::default(),
                temp: Cell::default(),
                buf: buffer,
                auto_wrap: true,
                report: VecDeque::new(),
                scroll_region: None,
                cursor_visible: true,
                alt_saved: None,
                g0_dec: false,
            },
        }
    }

    /// Write a single `byte` to console
    pub fn write_byte(&mut self, byte: u8) {
        self.parser
            .advance(&mut Performer::new(&mut self.inner), byte);
    }

    /// Read result for some commands
    pub fn pop_report(&mut self) -> Option<u8> {
        self.inner.report.pop_front()
    }

    /// Get mutable reference to the underlying TextBuffer.
    pub fn buf_mut(&mut self) -> &mut T {
        &mut self.inner.buf
    }

    /// Number of rows
    pub fn rows(&self) -> usize {
        self.inner.buf.height()
    }

    /// Number of columns
    pub fn columns(&self) -> usize {
        self.inner.buf.width()
    }

    /// Current cursor position as `(row, col)` in character cells.
    ///
    /// This is the authoritative logical cursor maintained while parsing the
    /// escape stream (updated by `goto` / `move_*` / `input` / ...), as opposed
    /// to any position a [`TextBuffer`] might infer from `write` calls alone.
    pub fn cursor(&self) -> (usize, usize) {
        (self.inner.cursor.row, self.inner.cursor.col)
    }

    /// Whether the text cursor should be drawn (DECTCEM / `?25`). Full-screen
    /// TUIs (nano, vim, htop) hide the cursor while repainting; the renderer
    /// passes this to `present` so a hidden cursor is not drawn or blinked.
    pub fn cursor_visible(&self) -> bool {
        self.inner.cursor_visible
    }
}

impl<T: TextBuffer> fmt::Write for Console<T> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

impl<T: TextBuffer> ConsoleInner<T> {
    /// The active scroll region `(top, bottom)` inclusive (0-indexed). Defaults
    /// to the whole screen when no region has been set.
    #[inline]
    fn region(&self) -> (usize, usize) {
        match self.scroll_region {
            Some((t, b)) => (t, b),
            None => (0, self.buf.height().saturating_sub(1)),
        }
    }

    /// Scroll the active region up by `count` lines (content moves up, blank
    /// lines appear at the bottom). When the region spans the whole screen this
    /// uses the buffer's fast `new_line` (which also records scrollback), so the
    /// normal shell-output path is byte-for-byte unchanged. A partial region is
    /// delegated to the buffer's `scroll_region_up`, which a framebuffer backend
    /// can implement as a bulk pixel copy rather than cell-by-cell.
    fn scroll_region_up(&mut self, count: usize) {
        let (top, bottom) = self.region();
        if bottom < top {
            return;
        }
        if top == 0 && bottom == self.buf.height().saturating_sub(1) {
            for _ in 0..count {
                self.buf.new_line(self.temp);
            }
        } else {
            let blank = self.temp.bg();
            self.buf.scroll_region_up(top, bottom, count, blank);
        }
    }

    /// Scroll the active region down by `count` lines (content moves down, blank
    /// lines appear at the top).
    fn scroll_region_down(&mut self, count: usize) {
        let (top, bottom) = self.region();
        if bottom < top {
            return;
        }
        let blank = self.temp.bg();
        self.buf.scroll_region_down(top, bottom, count, blank);
    }

    /// Switch to a blank alternate screen, saving the current screen and cursor.
    /// The scrollback history is left intact (we clear cells in place rather than
    /// via `buf.clear`, which would wipe history).
    fn enter_alt_screen(&mut self) {
        if self.alt_saved.is_some() {
            return;
        }
        let h = self.buf.height();
        let w = self.buf.width();
        let mut cells = alloc::vec::Vec::with_capacity(h);
        for r in 0..h {
            let mut row = alloc::vec::Vec::with_capacity(w);
            for c in 0..w {
                row.push(self.buf.read(r, c));
            }
            cells.push(row);
        }
        self.alt_saved = Some(AltScreen {
            cells,
            cursor: self.cursor,
        });
        let bg = self.temp.bg();
        for r in 0..h {
            for c in 0..w {
                self.buf.write(r, c, bg);
            }
        }
        self.scroll_region = None;
        self.cursor = Cursor::default();
    }

    /// Restore the main screen and cursor saved by `enter_alt_screen`.
    fn exit_alt_screen(&mut self) {
        if let Some(saved) = self.alt_saved.take() {
            let h = self.buf.height().min(saved.cells.len());
            for r in 0..h {
                let row = &saved.cells[r];
                let w = self.buf.width().min(row.len());
                for c in 0..w {
                    self.buf.write(r, c, row[c]);
                }
            }
            self.cursor = saved.cursor;
            self.scroll_region = None;
        }
    }
}

impl<T: TextBuffer> Handler for ConsoleInner<T> {
    #[inline]
    fn input(&mut self, c: char) {
        // DEC Special Graphics: translate VT100 line-drawing bytes to Unicode.
        let c = if self.g0_dec {
            dec_special_char(c).unwrap_or(c)
        } else {
            c
        };
        trace!("  [input]: {:?} @ {:?}", c, self.cursor);
        if self.cursor.col >= self.buf.width() {
            if !self.auto_wrap {
                // skip this one
                return;
            }
            self.cursor.col = 0;
            self.linefeed();
        }
        let mut temp = self.temp;
        temp.c = c;
        self.buf.write(self.cursor.row, self.cursor.col, temp);
        self.cursor.col += 1;
    }

    #[inline]
    fn goto(&mut self, row: usize, col: usize) {
        trace!("Going to: line={}, col={}", row, col);
        self.cursor.row = min(row, self.buf.height());
        self.cursor.col = min(col, self.buf.width());
    }

    #[inline]
    fn goto_line(&mut self, row: usize) {
        trace!("Going to line: {}", row);
        self.goto(row, self.cursor.col)
    }

    #[inline]
    fn goto_col(&mut self, col: usize) {
        trace!("Going to column: {}", col);
        self.goto(self.cursor.row, col)
    }

    #[inline]
    fn move_up(&mut self, rows: usize) {
        trace!("Moving up: {}", rows);
        self.goto(self.cursor.row.saturating_sub(rows), self.cursor.col)
    }

    #[inline]
    fn move_down(&mut self, rows: usize) {
        trace!("Moving down: {}", rows);
        self.goto(
            min(self.cursor.row + rows, self.buf.height() - 1) as _,
            self.cursor.col,
        )
    }

    #[inline]
    fn move_forward(&mut self, cols: usize) {
        trace!("Moving forward: {}", cols);
        self.cursor.col = min(self.cursor.col + cols, self.buf.width() - 1);
    }

    #[inline]
    fn move_backward(&mut self, cols: usize) {
        trace!("Moving backward: {}", cols);
        self.cursor.col = self.cursor.col.saturating_sub(cols);
    }

    #[inline]
    fn move_down_and_cr(&mut self, rows: usize) {
        trace!("Moving down and cr: {}", rows);
        self.goto(min(self.cursor.row + rows, self.buf.height() - 1) as _, 0)
    }

    #[inline]
    fn move_up_and_cr(&mut self, rows: usize) {
        trace!("Moving up and cr: {}", rows);
        self.goto(self.cursor.row.saturating_sub(rows), 0)
    }

    #[inline]
    fn put_tab(&mut self, count: u16) {
        let mut count = count;
        let bg = self.temp.bg();
        while self.cursor.col < self.buf.width() && count > 0 {
            count -= 1;
            loop {
                self.buf.write(self.cursor.row, self.cursor.col, bg);
                self.cursor.col += 1;
                if self.cursor.col == self.buf.width() || self.cursor.col % 8 == 0 {
                    break;
                }
            }
        }
    }

    #[inline]
    fn backspace(&mut self) {
        trace!("Backspace");
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
    }

    #[inline]
    fn carriage_return(&mut self) {
        trace!("Carriage return");
        self.cursor.col = 0;
    }

    #[inline]
    fn linefeed(&mut self) {
        trace!("Linefeed");
        self.cursor.col = 0;
        let (_, bottom) = self.region();
        if self.cursor.row == bottom {
            // At the bottom of the scroll region: scroll it up by one. With the
            // default (full-screen) region this is exactly the old `new_line`.
            self.scroll_region_up(1);
        } else if self.cursor.row + 1 < self.buf.height() {
            self.cursor.row += 1;
        }
    }

    #[inline]
    fn scroll_up(&mut self, rows: usize) {
        self.scroll_region_up(rows);
    }

    #[inline]
    fn scroll_down(&mut self, rows: usize) {
        self.scroll_region_down(rows);
    }

    #[inline]
    fn insert_blank(&mut self, count: usize) {
        let width = self.buf.width();
        let row = self.cursor.row;
        let col = self.cursor.col;
        if col >= width {
            return;
        }
        let bg = self.temp.bg();
        let count = count.min(width - col);
        for c in (col..width).rev() {
            let cell = if c >= col + count {
                self.buf.read(row, c - count)
            } else {
                bg
            };
            self.buf.write(row, c, cell);
        }
    }

    #[inline]
    fn insert_blank_lines(&mut self, count: usize) {
        // IL opens `count` blank lines at the cursor: scroll the rows from the
        // cursor to the region bottom *down* by `count`.
        let (top, bottom) = self.region();
        let row = self.cursor.row;
        if row < top || row > bottom {
            return;
        }
        let blank = self.temp.bg();
        self.buf.scroll_region_down(row, bottom, count, blank);
        self.cursor.col = 0;
    }

    #[inline]
    fn delete_lines(&mut self, count: usize) {
        // DL removes `count` lines at the cursor: scroll the rows from the
        // cursor to the region bottom *up* by `count`.
        let (top, bottom) = self.region();
        let row = self.cursor.row;
        if row < top || row > bottom {
            return;
        }
        let blank = self.temp.bg();
        self.buf.scroll_region_up(row, bottom, count, blank);
        self.cursor.col = 0;
    }

    #[inline]
    fn reverse_index(&mut self) {
        let (top, _) = self.region();
        if self.cursor.row == top {
            self.scroll_region_down(1);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
        }
    }

    #[inline]
    fn erase_chars(&mut self, count: usize) {
        trace!("Erasing chars: count={}, col={}", count, self.cursor.col);

        let start = self.cursor.col;
        let end = min(start + count, self.buf.width());

        // Cleared cells have current background color set.
        let bg = self.temp.bg();
        for i in start..end {
            self.buf.write(self.cursor.row, i, bg);
        }
    }
    #[inline]
    fn delete_chars(&mut self, count: usize) {
        let columns = self.buf.width();
        let count = min(count, columns - self.cursor.col - 1);
        let row = self.cursor.row;

        let start = self.cursor.col;
        let end = start + count;

        let bg = self.temp.bg();
        for i in end..columns {
            self.buf.write(row, i - count, self.buf.read(row, i));
            self.buf.write(row, i, bg);
        }
    }

    /// Save current cursor position.
    fn save_cursor_position(&mut self) {
        trace!("Saving cursor position");
        self.saved_cursor = self.cursor;
    }

    /// Restore cursor position.
    fn restore_cursor_position(&mut self) {
        trace!("Restoring cursor position");
        self.cursor = self.saved_cursor;
    }

    #[inline]
    fn clear_line(&mut self, mode: LineClearMode) {
        trace!("Clearing line: {:?}", mode);
        let bg = self.temp.bg();
        match mode {
            LineClearMode::Right => {
                for i in self.cursor.col..self.buf.width() {
                    self.buf.write(self.cursor.row, i, bg);
                }
            }
            LineClearMode::Left => {
                for i in 0..=self.cursor.col {
                    self.buf.write(self.cursor.row, i, bg);
                }
            }
            LineClearMode::All => {
                for i in 0..self.buf.width() {
                    self.buf.write(self.cursor.row, i, bg);
                }
            }
        }
    }

    #[inline]
    fn clear_screen(&mut self, mode: ClearMode) {
        trace!("Clearing screen: {:?}", mode);
        let bg = self.temp.bg();
        let row = self.cursor.row;
        let col = self.cursor.col;
        match mode {
            ClearMode::Above => {
                for i in 0..row {
                    for j in 0..self.buf.width() {
                        self.buf.write(i, j, bg);
                    }
                }
                for j in 0..col {
                    self.buf.write(row, j, bg);
                }
            }
            ClearMode::Below => {
                for j in col..self.buf.width() {
                    self.buf.write(row, j, bg);
                }
                for i in row + 1..self.buf.height() {
                    for j in 0..self.buf.width() {
                        self.buf.write(i, j, bg);
                    }
                }
            }
            ClearMode::All => {
                self.buf.clear(bg);
                self.cursor = Cursor::default();
            }
            _ => {}
        }
    }

    #[inline]
    fn terminal_attribute(&mut self, attr: Attr) {
        trace!("Setting attribute: {:?}", attr);
        match attr {
            Attr::Foreground(color) => self.temp.fg = color,
            Attr::Background(color) => self.temp.bg = color,
            Attr::Reset => self.temp = Cell::default(),
            Attr::Reverse => self.temp.flags |= Flags::INVERSE,
            Attr::CancelReverse => self.temp.flags.remove(Flags::INVERSE),
            Attr::Bold => self.temp.flags.insert(Flags::BOLD),
            Attr::CancelBold => self.temp.flags.remove(Flags::BOLD),
            Attr::Dim => self.temp.flags.insert(Flags::DIM),
            Attr::CancelBoldDim => self.temp.flags.remove(Flags::BOLD | Flags::DIM),
            Attr::Italic => self.temp.flags.insert(Flags::ITALIC),
            Attr::CancelItalic => self.temp.flags.remove(Flags::ITALIC),
            Attr::Underline => self.temp.flags.insert(Flags::UNDERLINE),
            Attr::CancelUnderline => self.temp.flags.remove(Flags::UNDERLINE),
            Attr::Hidden => self.temp.flags.insert(Flags::HIDDEN),
            Attr::CancelHidden => self.temp.flags.remove(Flags::HIDDEN),
            Attr::Strike => self.temp.flags.insert(Flags::STRIKEOUT),
            Attr::CancelStrike => self.temp.flags.remove(Flags::STRIKEOUT),
            _ => {
                debug!("Term got unhandled attr: {:?}", attr);
            }
        }
    }

    #[inline]
    fn set_mode(&mut self, mode: Mode) {
        match mode {
            Mode::LineWrap => self.auto_wrap = true,
            Mode::ShowCursor => self.cursor_visible = true,
            Mode::SwapScreenAndSetRestoreCursor | Mode::SwapScreenBuffer | Mode::SwapScreenOld => {
                self.enter_alt_screen()
            }
            _ => debug!("[Unhandled CSI] Setting mode: {:?}", mode),
        }
    }

    #[inline]
    fn unset_mode(&mut self, mode: Mode) {
        match mode {
            Mode::LineWrap => self.auto_wrap = false,
            Mode::ShowCursor => self.cursor_visible = false,
            Mode::SwapScreenAndSetRestoreCursor | Mode::SwapScreenBuffer | Mode::SwapScreenOld => {
                self.exit_alt_screen()
            }
            _ => debug!("[Unhandled CSI] Unsetting mode: {:?}", mode),
        }
    }

    #[inline]
    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        // Params are 1-indexed; `top` is already defaulted to 1 by the parser.
        let height = self.buf.height();
        let bottom = bottom.unwrap_or(height);
        let t = top.saturating_sub(1);
        let b = bottom.saturating_sub(1).min(height.saturating_sub(1));
        // A valid region needs at least two lines; anything else resets to the
        // whole screen (matching xterm).
        self.scroll_region = if t < b { Some((t, b)) } else { None };
        // DECSTBM homes the cursor.
        self.cursor = Cursor::default();
    }

    #[inline]
    fn set_g0_charset(&mut self, dec: bool) {
        self.g0_dec = dec;
    }

    #[inline]
    fn device_status(&mut self, arg: usize) {
        trace!("Reporting device status: {}", arg);
        match arg {
            5 => {
                for &c in b"\x1b[0n" {
                    self.report.push_back(c);
                }
            }
            6 => {
                let s = alloc::format!("\x1b[{};{}R", self.cursor.row + 1, self.cursor.col + 1);
                for c in s.bytes() {
                    self.report.push_back(c);
                }
            }
            _ => debug!("unknown device status query: {}", arg),
        }
    }
}

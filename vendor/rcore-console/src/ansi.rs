use alloc::string::String;
use core::convert::TryFrom;

use vte::{Params, ParamsIter, Perform};

use crate::cell::Cell;
use crate::color::{Color, NamedColor, Rgb888};

/// Terminal modes.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Eq, PartialEq)]
pub enum Mode {
    /// ?1
    CursorKeys = 1,
    /// Select 80 or 132 columns per page (DECCOLM).
    ///
    /// CSI ? 3 h -> set 132 column font.
    /// CSI ? 3 l -> reset 80 column font.
    ///
    /// Additionally,
    ///
    /// * set margins to default positions
    /// * erases all data in page memory
    /// * resets DECLRMM to unavailable
    /// * clears data from the status line (if set to host-writable)
    ColumnMode = 3,
    /// IRM Insert Mode.
    ///
    /// NB should be part of non-private mode enum.
    ///
    /// * `CSI 4 h` change to insert mode
    /// * `CSI 4 l` reset to replacement mode
    Insert = 4,
    /// ?6
    Origin = 6,
    /// ?7
    LineWrap = 7,
    /// ?12
    BlinkingCursor = 12,
    /// 20
    ///
    /// NB This is actually a private mode. We should consider adding a second
    /// enumeration for public/private modesets.
    LineFeedNewLine = 20,
    /// ?25
    ShowCursor = 25,
    /// ?47 - use the alternate screen buffer (legacy, no cursor save/clear).
    SwapScreenOld = 47,
    /// ?1000
    ReportMouseClicks = 1000,
    /// ?1002
    ReportCellMouseMotion = 1002,
    /// ?1003
    ReportAllMouseMotion = 1003,
    /// ?1004
    ReportFocusInOut = 1004,
    /// ?1005
    Utf8Mouse = 1005,
    /// ?1006
    SgrMouse = 1006,
    /// ?1007
    AlternateScroll = 1007,
    /// ?1042
    UrgencyHints = 1042,
    /// ?1047 - use the alternate screen buffer.
    SwapScreenBuffer = 1047,
    /// ?1049
    SwapScreenAndSetRestoreCursor = 1049,
    /// ?2004
    BracketedPaste = 2004,
}

impl Mode {
    /// Create mode from a primitive.
    pub fn from_primitive(intermediate: Option<&u8>, num: u16) -> Option<Mode> {
        let private = match intermediate {
            Some(b'?') => true,
            None => false,
            _ => return None,
        };

        if private {
            Some(match num {
                1 => Mode::CursorKeys,
                3 => Mode::ColumnMode,
                6 => Mode::Origin,
                7 => Mode::LineWrap,
                12 => Mode::BlinkingCursor,
                25 => Mode::ShowCursor,
                47 => Mode::SwapScreenOld,
                1000 => Mode::ReportMouseClicks,
                1002 => Mode::ReportCellMouseMotion,
                1003 => Mode::ReportAllMouseMotion,
                1004 => Mode::ReportFocusInOut,
                1005 => Mode::Utf8Mouse,
                1006 => Mode::SgrMouse,
                1007 => Mode::AlternateScroll,
                1042 => Mode::UrgencyHints,
                1047 => Mode::SwapScreenBuffer,
                1049 => Mode::SwapScreenAndSetRestoreCursor,
                2004 => Mode::BracketedPaste,
                _ => {
                    trace!("[unimplemented] primitive mode: {}", num);
                    return None;
                }
            })
        } else {
            Some(match num {
                4 => Mode::Insert,
                20 => Mode::LineFeedNewLine,
                _ => return None,
            })
        }
    }
}

/// Mode for clearing line.
///
/// Relative to cursor.
#[derive(Debug)]
pub enum LineClearMode {
    /// Clear right of cursor.
    Right,
    /// Clear left of cursor.
    Left,
    /// Clear entire line.
    All,
}

/// Mode for clearing terminal.
///
/// Relative to cursor.
#[derive(Debug)]
pub enum ClearMode {
    /// Clear below cursor.
    Below,
    /// Clear above cursor.
    Above,
    /// Clear entire terminal.
    All,
    /// Clear 'saved' lines (scrollback).
    Saved,
}

/// Terminal character attributes.
#[derive(Debug, Eq, PartialEq)]
pub enum Attr {
    /// Clear all special abilities.
    Reset,
    /// Bold text.
    Bold,
    /// Dim or secondary color.
    Dim,
    /// Italic text.
    Italic,
    /// Underline text.
    Underline,
    /// Underlined twice.
    DoubleUnderline,
    /// Blink cursor slowly.
    BlinkSlow,
    /// Blink cursor fast.
    BlinkFast,
    /// Invert colors.
    Reverse,
    /// Do not display characters.
    Hidden,
    /// Strikeout text.
    Strike,
    /// Cancel bold.
    CancelBold,
    /// Cancel bold and dim.
    CancelBoldDim,
    /// Cancel italic.
    CancelItalic,
    /// Cancel all underlines.
    CancelUnderline,
    /// Cancel blink.
    CancelBlink,
    /// Cancel inversion.
    CancelReverse,
    /// Cancel text hiding.
    CancelHidden,
    /// Cancel strikeout.
    CancelStrike,
    /// Set indexed foreground color.
    Foreground(Color),
    /// Set indexed background color.
    Background(Color),
}

/// Type that handles actions from the parser.
///
/// XXX Should probably not provide default impls for everything, but it makes
/// writing specific handler impls for tests far easier.
pub trait Handler {
    /// A character to be displayed.
    fn input(&mut self, _c: char) {}

    /// Set cursor to position.
    fn goto(&mut self, _row: usize, _col: usize) {}

    /// Set cursor to specific row.
    fn goto_line(&mut self, _row: usize) {}

    /// Set cursor to specific column.
    fn goto_col(&mut self, _col: usize) {}

    /// Move cursor up `rows`.
    fn move_up(&mut self, _rows: usize) {}

    /// Move cursor down `rows`.
    fn move_down(&mut self, _rows: usize) {}

    /// Move cursor forward `cols`.
    fn move_forward(&mut self, _cols: usize) {}

    /// Move cursor backward `cols`.
    fn move_backward(&mut self, _cols: usize) {}

    /// Move cursor down `rows` and set to column 1.
    fn move_down_and_cr(&mut self, _rows: usize) {}

    /// Move cursor up `rows` and set to column 1.
    fn move_up_and_cr(&mut self, _rows: usize) {}

    /// Put `count` tabs.
    fn put_tab(&mut self, _count: u16) {}

    /// Backspace `count` characters.
    fn backspace(&mut self) {}

    /// Carriage return.
    fn carriage_return(&mut self) {}

    /// Linefeed.
    fn linefeed(&mut self) {}

    /// Scroll up `rows` rows.
    fn scroll_up(&mut self, _rows: usize) {}

    /// Scroll down `rows` rows.
    fn scroll_down(&mut self, _rows: usize) {}

    /// Insert `count` blank characters at the cursor, shifting the rest of the
    /// line right (ICH, `CSI @`).
    fn insert_blank(&mut self, _count: usize) {}

    /// Insert `count` blank lines at the cursor, pushing lines below down within
    /// the scroll region (IL, `CSI L`).
    fn insert_blank_lines(&mut self, _count: usize) {}

    /// Delete `count` lines at the cursor, pulling lines below up within the
    /// scroll region (DL, `CSI M`).
    fn delete_lines(&mut self, _count: usize) {}

    /// Reverse Index (RI, `ESC M`): move the cursor up one line, scrolling the
    /// scroll region down when already at its top.
    fn reverse_index(&mut self) {}

    /// Erase `count` chars in current line following cursor.
    ///
    /// Erase means resetting to the default state (default colors, no content,
    /// no mode flags).
    fn erase_chars(&mut self, _count: usize) {}

    /// Delete `count` chars.
    ///
    /// Deleting a character is like the delete key on the keyboard - everything
    /// to the right of the deleted things is shifted left.
    fn delete_chars(&mut self, _count: usize) {}

    /// Save current cursor position.
    fn save_cursor_position(&mut self) {}

    /// Restore cursor position.
    fn restore_cursor_position(&mut self) {}

    /// Clear current line.
    fn clear_line(&mut self, _mode: LineClearMode) {}

    /// Clear screen.
    fn clear_screen(&mut self, _mode: ClearMode) {}

    /// Set a terminal attribute.
    fn terminal_attribute(&mut self, _attr: Attr) {}

    /// Set mode.
    fn set_mode(&mut self, _mode: Mode) {}

    /// Unset mode.
    fn unset_mode(&mut self, _mode: Mode) {}

    /// DECSTBM - Set the terminal scrolling region.
    fn set_scrolling_region(&mut self, _top: usize, _bottom: Option<usize>) {}

    /// Select the G0 charset: DEC Special Graphics (`ESC ( 0`) when `dec` is
    /// true, ASCII (`ESC ( B`) otherwise.
    fn set_g0_charset(&mut self, _dec: bool) {}

    /// Report device status.
    fn device_status(&mut self, _arg: usize) {}
}

pub struct Performer<'a, H: Handler> {
    handler: &'a mut H,
}

impl<'a, H: Handler> Performer<'a, H> {
    pub fn new(handler: &'a mut H) -> Self {
        Self { handler }
    }
}

/// Perform actions
impl<'a, H: Handler> Perform for Performer<'a, H> {
    #[inline]
    fn print(&mut self, c: char) {
        self.handler.input(c);
    }

    #[inline]
    fn execute(&mut self, byte: u8) {
        match byte {
            C0::HT => self.handler.put_tab(1),
            C0::BS => self.handler.backspace(),
            C0::CR => self.handler.carriage_return(),
            C0::LF | C0::VT | C0::FF => self.handler.linefeed(),
            _ => debug!("[unhandled] execute byte={:02x}", byte),
        }
    }

    #[inline]
    fn hook(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        debug!(
            "[unhandled hook] params={:?}, ints: {:?}, ignore: {:?}, action: {:?}",
            params, intermediates, ignore, action
        );
    }

    #[inline]
    fn put(&mut self, byte: u8) {
        debug!("[unhandled put] byte={:?}", byte);
    }

    #[inline]
    fn unhook(&mut self) {
        debug!("[unhandled unhook]");
    }

    #[inline]
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        fn unhandled(params: &[&[u8]]) {
            let mut buf = String::new();
            for items in params {
                buf.push('[');
                for item in *items {
                    buf.push_str(&format!("{:?},", *item as char));
                }
                buf.push_str("],");
            }
            debug!("[unhandled osc_dispatch]: [{}] at line {}", &buf, line!());
        }
        unhandled(params);
    }

    #[inline]
    fn csi_dispatch(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        has_ignored_intermediates: bool,
        action: char,
    ) {
        macro_rules! unhandled {
            () => {{
                warn!(
                    "[Unhandled CSI] action={:?}, params={:?}, intermediates={:?}",
                    action, params, intermediates
                );
            }};
        }

        if has_ignored_intermediates || intermediates.len() > 1 {
            unhandled!();
            return;
        }

        let handler = &mut self.handler;
        let mut params_iter = params.iter();
        let mut next_param_or = |default: u16| {
            params_iter
                .next()
                .map(|param| param[0])
                .filter(|&param| param != 0)
                .unwrap_or(default)
        };
        match (action, intermediates) {
            ('A', []) => handler.move_up(next_param_or(1) as usize),
            ('B', []) | ('e', []) => handler.move_down(next_param_or(1) as usize),
            ('C', []) | ('a', []) => handler.move_forward(next_param_or(1) as usize),
            ('D', []) => handler.move_backward(next_param_or(1) as usize),
            ('E', []) => handler.move_down_and_cr(next_param_or(1) as usize),
            ('F', []) => handler.move_up_and_cr(next_param_or(1) as usize),
            ('G', []) | ('`', []) => handler.goto_col(next_param_or(1) as usize - 1),
            ('H', []) | ('f', []) => {
                let y = next_param_or(1) as usize;
                let x = next_param_or(1) as usize;
                handler.goto(y - 1, x - 1);
            }
            ('J', []) => {
                let mode = match next_param_or(0) {
                    0 => ClearMode::Below,
                    1 => ClearMode::Above,
                    2 => ClearMode::All,
                    3 => ClearMode::Saved,
                    _ => {
                        unhandled!();
                        return;
                    }
                };

                handler.clear_screen(mode);
            }
            ('K', []) => {
                let mode = match next_param_or(0) {
                    0 => LineClearMode::Right,
                    1 => LineClearMode::Left,
                    2 => LineClearMode::All,
                    _ => {
                        unhandled!();
                        return;
                    }
                };

                handler.clear_line(mode);
            }
            ('@', []) => handler.insert_blank(next_param_or(1) as usize),
            ('L', []) => handler.insert_blank_lines(next_param_or(1) as usize),
            ('M', []) => handler.delete_lines(next_param_or(1) as usize),
            ('P', []) => handler.delete_chars(next_param_or(1) as usize),
            ('S', []) => handler.scroll_up(next_param_or(1) as usize),
            ('T', []) => handler.scroll_down(next_param_or(1) as usize),
            ('X', []) => handler.erase_chars(next_param_or(1) as usize),
            ('d', []) => handler.goto_line(next_param_or(1) as usize - 1),
            ('h', intermediates) => {
                for param in params_iter.map(|param| param[0]) {
                    match Mode::from_primitive(intermediates.get(0), param) {
                        Some(mode) => handler.set_mode(mode),
                        None => unhandled!(),
                    }
                }
            }
            ('l', intermediates) => {
                for param in params_iter.map(|param| param[0]) {
                    match Mode::from_primitive(intermediates.get(0), param) {
                        Some(mode) => handler.unset_mode(mode),
                        None => unhandled!(),
                    }
                }
            }
            ('m', _) => {
                if params.is_empty() {
                    handler.terminal_attribute(Attr::Reset);
                } else {
                    attrs_from_sgr_parameters(&mut params_iter, |attr| match attr {
                        Some(attr) => handler.terminal_attribute(attr),
                        None => unhandled!(),
                    });
                }
            }
            ('n', []) => handler.device_status(next_param_or(0) as usize),
            ('r', []) => {
                let top = next_param_or(1) as usize;
                let bottom = params_iter
                    .next()
                    .map(|param| param[0] as usize)
                    .filter(|&param| param != 0);

                handler.set_scrolling_region(top, bottom);
            }
            _ => unhandled!(),
        }
    }

    #[inline]
    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        macro_rules! unhandled {
            () => {{
                debug!(
                    "[unhandled] esc_dispatch ints={:?}, byte={:?} ({:02x})",
                    intermediates, byte as char, byte
                );
            }};
        }

        match (byte, intermediates) {
            (b'7', []) => self.handler.save_cursor_position(),
            (b'8', []) => self.handler.restore_cursor_position(),
            // IND (Index): move down one line, scrolling at the region bottom.
            // Our `linefeed` already does this (column reset is harmless for the
            // ncurses callers that emit it after a carriage return).
            (b'D', []) => self.handler.linefeed(),
            // NEL (Next Line): CR + LF.
            (b'E', []) => {
                self.handler.carriage_return();
                self.handler.linefeed();
            }
            // RI (Reverse Index): move up one line, scrolling at the region top.
            (b'M', []) => self.handler.reverse_index(),
            // G0 charset: DEC Special Graphics (line drawing) vs ASCII.
            (b'0', [b'(']) => self.handler.set_g0_charset(true),
            (b'B', [b'(']) => self.handler.set_g0_charset(false),
            // G1 charset (`ESC ) x`) is accepted but unused (apps draw via G0).
            (b'0', [b')']) | (b'B', [b')']) => {}
            _ => unhandled!(),
        }
    }
}

#[inline]
fn attrs_from_sgr_parameters<F>(params: &mut ParamsIter<'_>, mut handler: F)
where
    F: FnMut(Option<Attr>),
{
    while let Some(param) = params.next() {
        let attr = match param {
            [0] => Some(Attr::Reset),
            [1] => Some(Attr::Bold),
            [2] => Some(Attr::Dim),
            [3] => Some(Attr::Italic),
            [4, 0] => Some(Attr::CancelUnderline),
            [4, 2] => Some(Attr::DoubleUnderline),
            [4, ..] => Some(Attr::Underline),
            [5] => Some(Attr::BlinkSlow),
            [6] => Some(Attr::BlinkFast),
            [7] => Some(Attr::Reverse),
            [8] => Some(Attr::Hidden),
            [9] => Some(Attr::Strike),
            [21] => Some(Attr::CancelBold),
            [22] => Some(Attr::CancelBoldDim),
            [23] => Some(Attr::CancelItalic),
            [24] => Some(Attr::CancelUnderline),
            [25] => Some(Attr::CancelBlink),
            [27] => Some(Attr::CancelReverse),
            [28] => Some(Attr::CancelHidden),
            [29] => Some(Attr::CancelStrike),
            [30] => Some(Attr::Foreground(Color::Named(NamedColor::Black))),
            [31] => Some(Attr::Foreground(Color::Named(NamedColor::Red))),
            [32] => Some(Attr::Foreground(Color::Named(NamedColor::Green))),
            [33] => Some(Attr::Foreground(Color::Named(NamedColor::Yellow))),
            [34] => Some(Attr::Foreground(Color::Named(NamedColor::Blue))),
            [35] => Some(Attr::Foreground(Color::Named(NamedColor::Magenta))),
            [36] => Some(Attr::Foreground(Color::Named(NamedColor::Cyan))),
            [37] => Some(Attr::Foreground(Color::Named(NamedColor::White))),
            [38] => {
                let mut iter = params.map(|param| param[0]);
                parse_sgr_color(&mut iter).map(Attr::Foreground)
            }
            [38, params @ ..] => {
                let rgb_start = if params.len() > 4 { 2 } else { 1 };
                let rgb_iter = params[rgb_start..].iter().copied();
                let mut iter = core::iter::once(params[0]).chain(rgb_iter);

                parse_sgr_color(&mut iter).map(Attr::Foreground)
            }
            [39] => Some(Attr::Foreground(Cell::default().fg)),
            [40] => Some(Attr::Background(Color::Named(NamedColor::Black))),
            [41] => Some(Attr::Background(Color::Named(NamedColor::Red))),
            [42] => Some(Attr::Background(Color::Named(NamedColor::Green))),
            [43] => Some(Attr::Background(Color::Named(NamedColor::Yellow))),
            [44] => Some(Attr::Background(Color::Named(NamedColor::Blue))),
            [45] => Some(Attr::Background(Color::Named(NamedColor::Magenta))),
            [46] => Some(Attr::Background(Color::Named(NamedColor::Cyan))),
            [47] => Some(Attr::Background(Color::Named(NamedColor::White))),
            [48] => {
                let mut iter = params.map(|param| param[0]);
                parse_sgr_color(&mut iter).map(Attr::Background)
            }
            [48, params @ ..] => {
                let rgb_start = if params.len() > 4 { 2 } else { 1 };
                let rgb_iter = params[rgb_start..].iter().copied();
                let mut iter = core::iter::once(params[0]).chain(rgb_iter);

                parse_sgr_color(&mut iter).map(Attr::Background)
            }
            [49] => Some(Attr::Background(Cell::default().bg)),
            [90] => Some(Attr::Foreground(Color::Named(NamedColor::BrightBlack))),
            [91] => Some(Attr::Foreground(Color::Named(NamedColor::BrightRed))),
            [92] => Some(Attr::Foreground(Color::Named(NamedColor::BrightGreen))),
            [93] => Some(Attr::Foreground(Color::Named(NamedColor::BrightYellow))),
            [94] => Some(Attr::Foreground(Color::Named(NamedColor::BrightBlue))),
            [95] => Some(Attr::Foreground(Color::Named(NamedColor::BrightMagenta))),
            [96] => Some(Attr::Foreground(Color::Named(NamedColor::BrightCyan))),
            [97] => Some(Attr::Foreground(Color::Named(NamedColor::BrightWhite))),
            [100] => Some(Attr::Background(Color::Named(NamedColor::BrightBlack))),
            [101] => Some(Attr::Background(Color::Named(NamedColor::BrightRed))),
            [102] => Some(Attr::Background(Color::Named(NamedColor::BrightGreen))),
            [103] => Some(Attr::Background(Color::Named(NamedColor::BrightYellow))),
            [104] => Some(Attr::Background(Color::Named(NamedColor::BrightBlue))),
            [105] => Some(Attr::Background(Color::Named(NamedColor::BrightMagenta))),
            [106] => Some(Attr::Background(Color::Named(NamedColor::BrightCyan))),
            [107] => Some(Attr::Background(Color::Named(NamedColor::BrightWhite))),
            _ => None,
        };
        handler(attr);
    }
}

/// Parse a color specifier from list of attributes.
fn parse_sgr_color(params: &mut dyn Iterator<Item = u16>) -> Option<Color> {
    match params.next() {
        Some(2) => Some(Color::Spec(Rgb888::new(
            u8::try_from(params.next()?).ok()?,
            u8::try_from(params.next()?).ok()?,
            u8::try_from(params.next()?).ok()?,
        ))),
        Some(5) => Some(Color::Indexed(u8::try_from(params.next()?).ok()?)),
        _ => None,
    }
}

/// C0 set of 7-bit control characters (from ANSI X3.4-1977).
#[allow(dead_code)]
#[allow(non_snake_case)]
pub mod C0 {
    /// Null filler, terminal should ignore this character.
    pub const NUL: u8 = 0x00;
    /// Start of Header.
    pub const SOH: u8 = 0x01;
    /// Start of Text, implied end of header.
    pub const STX: u8 = 0x02;
    /// End of Text, causes some terminal to respond with ACK or NAK.
    pub const ETX: u8 = 0x03;
    /// End of Transmission.
    pub const EOT: u8 = 0x04;
    /// Enquiry, causes terminal to send ANSWER-BACK ID.
    pub const ENQ: u8 = 0x05;
    /// Acknowledge, usually sent by terminal in response to ETX.
    pub const ACK: u8 = 0x06;
    /// Bell, triggers the bell, buzzer, or beeper on the terminal.
    pub const BEL: u8 = 0x07;
    /// Backspace, can be used to define overstruck characters.
    pub const BS: u8 = 0x08;
    /// Horizontal Tabulation, move to next predetermined position.
    pub const HT: u8 = 0x09;
    /// Linefeed, move to same position on next line (see also NL).
    pub const LF: u8 = 0x0A;
    /// Vertical Tabulation, move to next predetermined line.
    pub const VT: u8 = 0x0B;
    /// Form Feed, move to next form or page.
    pub const FF: u8 = 0x0C;
    /// Carriage Return, move to first character of current line.
    pub const CR: u8 = 0x0D;
    /// Shift Out, switch to G1 (other half of character set).
    pub const SO: u8 = 0x0E;
    /// Shift In, switch to G0 (normal half of character set).
    pub const SI: u8 = 0x0F;
    /// Data Link Escape, interpret next control character specially.
    pub const DLE: u8 = 0x10;
    /// (DC1) Terminal is allowed to resume transmitting.
    pub const XON: u8 = 0x11;
    /// Device Control 2, causes ASR-33 to activate paper-tape reader.
    pub const DC2: u8 = 0x12;
    /// (DC2) Terminal must pause and refrain from transmitting.
    pub const XOFF: u8 = 0x13;
    /// Device Control 4, causes ASR-33 to deactivate paper-tape reader.
    pub const DC4: u8 = 0x14;
    /// Negative Acknowledge, used sometimes with ETX and ACK.
    pub const NAK: u8 = 0x15;
    /// Synchronous Idle, used to maintain timing in Sync communication.
    pub const SYN: u8 = 0x16;
    /// End of Transmission block.
    pub const ETB: u8 = 0x17;
    /// Cancel (makes VT100 abort current escape sequence if any).
    pub const CAN: u8 = 0x18;
    /// End of Medium.
    pub const EM: u8 = 0x19;
    /// Substitute (VT100 uses this to display parity errors).
    pub const SUB: u8 = 0x1A;
    /// Prefix to an escape sequence.
    pub const ESC: u8 = 0x1B;
    /// File Separator.
    pub const FS: u8 = 0x1C;
    /// Group Separator.
    pub const GS: u8 = 0x1D;
    /// Record Separator (sent by VT132 in block-transfer mode).
    pub const RS: u8 = 0x1E;
    /// Unit Separator.
    pub const US: u8 = 0x1F;
    /// Delete, should be ignored by terminal.
    pub const DEL: u8 = 0x7f;
}

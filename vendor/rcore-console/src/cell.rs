use crate::color::{Color, NamedColor};

bitflags::bitflags! {
    pub struct Flags: u16 {
        const INVERSE                   = 0b0000_0000_0000_0001;
        const BOLD                      = 0b0000_0000_0000_0010;
        const ITALIC                    = 0b0000_0000_0000_0100;
        const BOLD_ITALIC               = 0b0000_0000_0000_0110;
        const UNDERLINE                 = 0b0000_0000_0000_1000;
        const WRAPLINE                  = 0b0000_0000_0001_0000;
        const WIDE_CHAR                 = 0b0000_0000_0010_0000;
        const WIDE_CHAR_SPACER          = 0b0000_0000_0100_0000;
        const DIM                       = 0b0000_0000_1000_0000;
        const DIM_BOLD                  = 0b0000_0000_1000_0010;
        const HIDDEN                    = 0b0000_0001_0000_0000;
        const STRIKEOUT                 = 0b0000_0010_0000_0000;
        const LEADING_WIDE_CHAR_SPACER  = 0b0000_0100_0000_0000;
        const DOUBLE_UNDERLINE          = 0b0000_1000_0000_0000;
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Cell {
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
}

impl Cell {
    pub fn bg(&self) -> Self {
        Self {
            bg: self.bg,
            ..Default::default()
        }
    }
}

impl Default for Cell {
    #[inline]
    fn default() -> Cell {
        Cell {
            c: ' ',
            bg: Color::Named(NamedColor::Black),
            fg: Color::Named(NamedColor::BrightWhite),
            flags: Flags::empty(),
        }
    }
}

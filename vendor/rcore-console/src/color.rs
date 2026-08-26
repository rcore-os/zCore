pub use embedded_graphics::pixelcolor::Rgb888;

/// Standard colors.
///
/// The order here matters since the enum should be castable to a `usize` for
/// indexing a color list.
#[repr(u8)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub enum NamedColor {
    /// Black.
    Black = 0,
    /// Red.
    Red = 1,
    /// Green.
    Green = 2,
    /// Yellow.
    Yellow = 3,
    /// Blue.
    Blue = 4,
    /// Magenta.
    Magenta = 5,
    /// Cyan.
    Cyan = 6,
    /// White.
    White = 7,
    /// Bright black.
    BrightBlack = 8,
    /// Bright red.
    BrightRed = 9,
    /// Bright green.
    BrightGreen = 10,
    /// Bright yellow.
    BrightYellow = 11,
    /// Bright blue.
    BrightBlue = 12,
    /// Bright magenta.
    BrightMagenta = 13,
    /// Bright cyan.
    BrightCyan = 14,
    /// Bright white.
    BrightWhite = 15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Named(NamedColor),
    Spec(Rgb888),
    Indexed(u8),
}

impl Color {
    pub fn to_rgb(self) -> Rgb888 {
        match self {
            Color::Spec(rgb) => rgb,
            Color::Named(name) => COLOR_MAP[name as usize],
            Color::Indexed(idx) => COLOR_MAP[idx as usize],
        }
    }
}

lazy_static::lazy_static! {
    /// Array of indexed colors.
    ///
    /// | Indices  | Description       |
    /// | -------- | ----------------- |
    /// | 0..16    | Named ANSI colors |
    /// | 16..232  | Color cube        |
    /// | 233..256 | Grayscale ramp    |
    ///
    /// Reference: https://en.wikipedia.org/wiki/ANSI_escape_code#Colors
    static ref COLOR_MAP: [Rgb888; 256] = {
        let mut colors = [Rgb888::default(); 256];
        colors[NamedColor::Black as usize] = Rgb888::new(0, 0, 0);
        colors[NamedColor::Red as usize] = Rgb888::new(194, 54, 33);
        colors[NamedColor::Green as usize] = Rgb888::new(37, 188, 36);
        colors[NamedColor::Yellow as usize] = Rgb888::new(173, 173, 39);
        colors[NamedColor::Blue as usize] = Rgb888::new(73, 46, 225);
        colors[NamedColor::Magenta as usize] = Rgb888::new(211, 56, 211);
        colors[NamedColor::Cyan as usize] = Rgb888::new(51, 187, 200);
        colors[NamedColor::White as usize] = Rgb888::new(203, 204, 205);
        colors[NamedColor::BrightBlack as usize] = Rgb888::new(129, 131, 131);
        colors[NamedColor::BrightRed as usize] = Rgb888::new(252, 57, 31);
        colors[NamedColor::BrightGreen as usize] = Rgb888::new(49, 231, 34);
        colors[NamedColor::BrightYellow as usize] = Rgb888::new(234, 236, 35);
        colors[NamedColor::BrightBlue as usize] = Rgb888::new(88, 51, 255);
        colors[NamedColor::BrightMagenta as usize] = Rgb888::new(249, 53, 248);
        colors[NamedColor::BrightCyan as usize] = Rgb888::new(20, 240, 240);
        colors[NamedColor::BrightWhite as usize] = Rgb888::new(233, 235, 235);

        for r in 0..6 {
            for g in 0..6 {
                for b in 0..6 {
                    let index = 16 + 36 * r + 6 * g + b;
                    let f = |c: usize| if c == 0 { 0 } else { (c * 40 + 55) as u8 };
                    colors[index] = Rgb888::new(f(r), f(g), f(b));
                }
            }
        }

        for i in 0..24 {
            let index = 16 + 216 + i;
            let c = (i * 10 + 8) as u8;
            colors[index] = Rgb888::new(c, c, c);
        }

        colors
    };
}

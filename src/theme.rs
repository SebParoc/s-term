//! Colours. The themes themselves (presets, derived UI colours, egui visuals) live in the shared
//! `s-style` crate (its own repo, ../s-style), which S-study uses too; this module adds what only a terminal
//! needs: looking up the 256-colour table and the colours programs set at runtime (OSC 4/10/11).

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb as TermRgb};

pub use s_style::theme::*;

fn from_term(c: TermRgb) -> Rgb {
    Rgb { r: c.r, g: c.g, b: c.b }
}

/// Terminal colour lookups on a [`Theme`].
pub trait TermColors {
    /// Default value of a color index (0..16 palette, 16..256 cube/gray, 256+ named specials).
    fn default_index(&self, i: usize) -> Rgb;
    /// Color index, honoring runtime overrides set by programs (OSC 4/10/11).
    fn index(&self, i: usize, colors: &Colors) -> Rgb;
    fn resolve(&self, c: Color, colors: &Colors, bold_bright: bool) -> Rgb;
}

impl TermColors for Theme {
    fn default_index(&self, i: usize) -> Rgb {
        match i {
            0..=15 => self.palette[i],
            16..=231 => {
                let n = i - 16;
                let lvl = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
                Rgb { r: lvl(n / 36), g: lvl((n / 6) % 6), b: lvl(n % 6) }
            }
            232..=255 => {
                let v = (8 + (i - 232) * 10) as u8;
                Rgb { r: v, g: v, b: v }
            }
            _ => match i {
                x if x == NamedColor::Background as usize => self.bg,
                x if x == NamedColor::Cursor as usize => self.cursor,
                x if x == NamedColor::DimForeground as usize => scale(self.fg, 0.66),
                x if x >= NamedColor::DimBlack as usize && x <= NamedColor::DimWhite as usize => {
                    scale(self.palette[x - NamedColor::DimBlack as usize], 0.66)
                }
                _ => self.fg,
            },
        }
    }

    fn index(&self, i: usize, colors: &Colors) -> Rgb {
        colors[i].map(from_term).unwrap_or_else(|| self.default_index(i))
    }

    fn resolve(&self, c: Color, colors: &Colors, bold_bright: bool) -> Rgb {
        match c {
            Color::Spec(rgb) => from_term(rgb),
            Color::Indexed(i) => {
                let i = i as usize;
                self.index(if bold_bright && i < 8 { i + 8 } else { i }, colors)
            }
            Color::Named(n) => {
                let i = n as usize;
                self.index(if bold_bright && i < 8 { i + 8 } else { i }, colors)
            }
        }
    }
}

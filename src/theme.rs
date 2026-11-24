use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use egui::Color32;

pub const fn rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

pub fn c32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

fn scale(c: Rgb, f: f32) -> Rgb {
    Rgb { r: (c.r as f32 * f) as u8, g: (c.g as f32 * f) as u8, b: (c.b as f32 * f) as u8 }
}

/// A complete color scheme: terminal colors plus the UI chrome derived from them.
pub struct Theme {
    pub id: &'static str,
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    pub palette: [Rgb; 16],
    pub accent: Color32,
    pub accent2: Color32,
    /// Window chrome (title bar, status bar, rail).
    pub bar: Color32,
    /// Side panel background.
    pub panel: Color32,
    /// Cards, popups, menus.
    pub elevated: Color32,
    /// Subtle surface (hover, inactive headers).
    pub header: Color32,
    /// Active / selected surface.
    pub header_focused: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub selection: Color32,
    /// Glass: hairline rim, top-edge light, bottom-edge shade and the diagonal sheen tint.
    pub rim: Color32,
    pub edge_light: Color32,
    pub edge_shade: Color32,
    pub gloss: Color32,
    /// Translucent glass fill for floating panels.
    pub glass: Color32,
    /// The raised light pill marking the selected item, and the ink on it.
    pub pick: Color32,
    pub pick_ink: Color32,
    /// A third, warm accent for the backdrop and attention states.
    pub warm: Color32,
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r, b.r), l(a.g, b.g), l(a.b, b.b))
}

const BLACK: Rgb = rgb(0x000000);

/// (id, display name) of the built-in themes.
pub const PRESETS: &[(&str, &str)] = &[
    ("neon", "Neon"),
    ("midnight", "Midnight"),
    ("ember", "Ember"),
    ("aurora", "Aurora"),
    ("graphite", "Graphite"),
    ("atelier", "Atelier"),
    ("full-moon", "Full Moon"),
];

impl Theme {
    #[allow(clippy::too_many_arguments)]
    fn build(id: &'static str, bg: u32, fg: u32, cursor: u32, accent: u32, accent2: u32, palette: [u32; 16]) -> Self {
        let (bg, fg) = (rgb(bg), rgb(fg));
        let accent_rgb = rgb(accent);
        let a = c32(accent_rgb);
        let alpha = |c: Rgb, a: u8| Color32::from_rgba_unmultiplied(c.r, c.g, c.b, a);
        let glass = mix(bg, fg, 0.06);
        Self {
            id,
            fg,
            bg,
            cursor: rgb(cursor),
            palette: palette.map(rgb),
            accent: a,
            accent2: c32(rgb(accent2)),
            bar: mix(bg, BLACK, 0.38),
            panel: mix(bg, BLACK, 0.22),
            elevated: mix(bg, fg, 0.07),
            header: mix(bg, fg, 0.06),
            header_focused: mix(bg, accent_rgb, 0.2),
            border: mix(bg, fg, 0.11),
            text: mix(fg, rgb(0xffffff), 0.35),
            muted: mix(bg, fg, 0.5),
            selection: Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 0x55),
            rim: alpha(fg, 34),
            edge_light: alpha(rgb(0xffffff), 34),
            edge_shade: Color32::from_black_alpha(50),
            gloss: alpha(fg, 5),
            glass: Color32::from_rgba_unmultiplied(glass.r(), glass.g(), glass.b(), 200),
            pick: Color32::from_rgba_unmultiplied(glass.r(), glass.g(), glass.b(), 200),
            pick_ink: mix(fg, rgb(0xffffff), 0.35),
            warm: c32(rgb(palette[3])),
        }
    }

    pub fn preset(id: &str) -> Self {
        match id {
            // Warm espresso and cream, with coral, teal and ochre: a studio-lit glass look.
            "atelier" => Self::build("atelier", 0x1c1512, 0xf6e9da, 0xff9177, 0xff9177, 0x75dac0, [
                0x2e2420, 0xff8a70, 0x75dac0, 0xf1be58, 0x9fb7d9, 0xe1a6c9, 0x8fd3c9, 0xe5d5c7,
                0x6b5a50, 0xffab98, 0x9be8d3, 0xf6d28a, 0xbccde6, 0xecc0da, 0xaee3db, 0xfff6e9,
            ]),
            // Moonlight on charcoal: white and grayscale, keeping only a soft red/green for diffs and errors.
            "full-moon" => Self::build("full-moon", 0x0e0e0f, 0xd6d6d6, 0xf5f5f5, 0xf0f0f0, 0xa8a8a8, [
                0x1e1e1e, 0xe07a7a, 0x8fcf94, 0xe8e8e8, 0x8f8f8f, 0xbababa, 0xc4c4c4, 0xd6d6d6,
                0x5c5c5c, 0xeb9b9b, 0xa9dcad, 0xf4f4f4, 0xa8a8a8, 0xcccccc, 0xd8d8d8, 0xffffff,
            ]),
            "midnight" => Self::build("midnight", 0x0b0e14, 0xbfbdb6, 0xe6b450, 0x59c2ff, 0x95e6cb, [
                0x131721, 0xf07178, 0xaad94c, 0xffb454, 0x59c2ff, 0xd2a6ff, 0x95e6cb, 0xbfbdb6,
                0x565b66, 0xf26d78, 0xc2d94c, 0xffd580, 0x73b8ff, 0xdfbfff, 0xb8ffe9, 0xf8f8f2,
            ]),
            "ember" => Self::build("ember", 0x14110f, 0xe6dcd2, 0xd97757, 0xd97757, 0xf2c48d, [
                0x221d1a, 0xe5645a, 0xa9c47f, 0xe8b45a, 0x7fa7c9, 0xc78fb4, 0x86c1b9, 0xd6cabf,
                0x5c524b, 0xf07b70, 0xbdd895, 0xf5c676, 0x98bddc, 0xd9a6c6, 0x9fd6ce, 0xfaf4ee,
            ]),
            "aurora" => Self::build("aurora", 0x0b1315, 0xcde3de, 0x5eead4, 0x5eead4, 0xa78bfa, [
                0x14201f, 0xfb7185, 0x86efac, 0xfcd34d, 0x7dd3fc, 0xc4b5fd, 0x5eead4, 0xcbd5d1,
                0x4b5d5a, 0xfda4af, 0xbbf7d0, 0xfde68a, 0xbae6fd, 0xddd6fe, 0x99f6e4, 0xf8fafc,
            ]),
            "graphite" => Self::build("graphite", 0x111213, 0xd4d4d4, 0x8ab4f8, 0x8ab4f8, 0xc58af9, [
                0x1e1f20, 0xf28b82, 0x81c995, 0xfdd663, 0x8ab4f8, 0xc58af9, 0x78d9ec, 0xd4d4d4,
                0x5f6368, 0xf6aea9, 0xa8dab5, 0xfde293, 0xaecbfa, 0xd7aefb, 0xa1e4f2, 0xffffff,
            ]),
            _ => Self::build("neon", 0x0d0f17, 0xc8d3f5, 0xff79c6, 0xbb9af7, 0x7dcfff, [
                0x1b1e2b, 0xff5370, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xa6accd,
                0x545a72, 0xff757f, 0xd4f0a6, 0xffd88a, 0x9ab8ff, 0xd6aef7, 0xa3e7ff, 0xffffff,
            ]),
        }
    }

    /// egui widget colors that match this theme.
    pub fn apply_visuals(&self, ctx: &egui::Context) {
        let mut v = egui::Visuals::dark();
        v.panel_fill = self.panel;
        v.window_fill = self.elevated;
        v.window_stroke = egui::Stroke::new(1.0_f32, self.border);
        v.window_rounding = 10.0.into();
        v.menu_rounding = 8.0.into();
        v.extreme_bg_color = c32(self.bg);
        v.faint_bg_color = self.header;
        v.override_text_color = Some(self.text);
        v.hyperlink_color = self.accent2;
        v.selection.bg_fill = self.accent.gamma_multiply(0.45);
        v.selection.stroke = egui::Stroke::new(1.0_f32, self.accent);
        v.popup_shadow = egui::epaint::Shadow { offset: egui::vec2(0.0, 6.0), blur: 24.0, spread: 0.0, color: Color32::from_black_alpha(140) };
        v.window_shadow = v.popup_shadow;
        for (w, fill) in [
            (&mut v.widgets.noninteractive, self.elevated),
            (&mut v.widgets.inactive, self.header),
            (&mut v.widgets.hovered, self.header_focused),
            (&mut v.widgets.active, self.header_focused),
            (&mut v.widgets.open, self.header_focused),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.rounding = 6.0.into();
            w.bg_stroke = egui::Stroke::new(1.0_f32, self.border);
            w.fg_stroke.color = self.text;
        }
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, self.border);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, self.accent.gamma_multiply(0.6));
        ctx.set_visuals(v);
    }

    /// Default value of a color index (0..16 palette, 16..256 cube/gray, 256+ named specials).
    pub fn default_index(&self, i: usize) -> Rgb {
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

    /// Color index, honoring runtime overrides set by programs (OSC 4/10/11).
    pub fn index(&self, i: usize, colors: &Colors) -> Rgb {
        colors[i].unwrap_or_else(|| self.default_index(i))
    }

    pub fn resolve(&self, c: Color, colors: &Colors, bold_bright: bool) -> Rgb {
        match c {
            Color::Spec(rgb) => rgb,
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

    pub fn dim(c: Rgb) -> Rgb {
        scale(c, 0.66)
    }
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn lerp_rect(a: egui::Rect, b: egui::Rect, t: f32) -> egui::Rect {
    egui::Rect::from_min_max(a.min + (b.min - a.min) * t, a.max + (b.max - a.max) * t)
}

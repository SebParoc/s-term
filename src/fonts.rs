//! Fonts. S-term ships JetBrains Mono (terminal) and Inter (interface), both under the SIL Open
//! Font License (see `assets/fonts/`). Any installed monospace font can be picked in Settings,
//! or forced with `S_TERM_FONT="Family Name"`.

use std::collections::HashSet;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};

use egui::{FontData, FontDefinitions, FontFamily};

pub const MONO_BOLD: &str = "s-term-mono-bold";
/// The square egui draws for a character no font has. `has_glyph` can't tell it from a missing one.
pub const TOFU: char = '\u{25FB}';
/// The bundled terminal font (the default).
pub const BUNDLED_MONO: &str = "JetBrains Mono";

const JB_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const JB_BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf");
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");

/// (family, file, face index) of the best fontconfig match.
fn fc_match(pattern: &str) -> Option<(String, String, u32)> {
    let out = Command::new("fc-match").args(["-f", "%{family}\n%{file}\n%{index}", pattern]).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    let mut l = s.lines();
    let family = l.next()?.to_string();
    let file = l.next()?.to_string();
    let index = l.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    Some((family, file, index))
}

fn find(family: &str, style: &str) -> Option<(String, FontData)> {
    let (fam, file, index) = fc_match(&format!("{family}:style={style}"))?;
    // fc-match always returns *something*; make sure it is the family we asked for.
    if !fam.to_lowercase().contains(&family.to_lowercase()) {
        return None;
    }
    let bytes = std::fs::read(&file).ok()?;
    // egui panics on a font it can't parse (Type 1, bitmap-only, …), so check first.
    ab_glyph::FontRef::try_from_slice_and_index(&bytes, index).ok()?;
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    Some((file, data))
}

/// True when face `index` of the font file `data` parses and has a drawable glyph for `c`.
fn covers(data: &[u8], index: u32, c: char) -> bool {
    use ab_glyph::Font as _;
    let Ok(font) = ab_glyph::FontRef::try_from_slice_and_index(data, index) else { return false };
    let id = font.glyph_id(c);
    // Colour-bitmap emoji fonts map the character but have no outline, which egui would draw as nothing.
    id.0 != 0 && (c.is_whitespace() || font.outline(id).is_some())
}

/// The best installed font that can draw `c`: its file contents and face index. Monospace ones come first.
fn system_font_for(c: char) -> Option<(Vec<u8>, u32)> {
    let pattern = format!("monospace:charset={:x}", c as u32);
    let out = Command::new("fc-match").args(["-s", "-f", "%{file}\t%{index}\n", &pattern]).output().ok()?;
    // fontconfig sorts fonts that have the character first, so if the first few don't, none does.
    String::from_utf8_lossy(&out.stdout).lines().take(8).find_map(|l| {
        let (file, index) = l.split_once('\t')?;
        let index = index.parse::<u32>().ok()? & 0xFFFF;
        let bytes = std::fs::read(file).ok()?;
        covers(&bytes, index, c).then_some((bytes, index))
    })
}

/// System fonts loaded on demand for characters that neither the chosen font nor the bundled ones have:
/// Claude Code's ✢ ✳ ✶ ✻ ✽ spinner, CJK, Nerd Font icons and so on. Without them egui draws a square.
/// The lookup (`fc-match` and reading the font file) runs on the `s-term-fonts` thread.
pub struct Fallbacks {
    asked: HashSet<char>,
    tx: Sender<char>,
    rx: Receiver<FontData>,
    loaded: Vec<FontData>,
}

impl Fallbacks {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, chars) = channel::<char>();
        let (fonts, rx) = channel();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("s-term-fonts".into()).spawn(move || {
            let mut sent: Vec<(&'static [u8], u32)> = Vec::new();
            while let Ok(c) = chars.recv() {
                // A font found for an earlier character often has this one too (the UI asks before it's applied).
                if sent.iter().any(|&(data, index)| covers(data, index, c)) {
                    continue;
                }
                let Some((bytes, index)) = system_font_for(c) else { continue };
                // Fallbacks stay for the life of the app, and a static slice saves egui from copying
                // the whole file (tens of MB for CJK) every time the font list changes.
                let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
                sent.push((data, index));
                let mut font = FontData::from_static(data);
                font.index = index;
                if fonts.send(font).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        Self { asked: HashSet::new(), tx, rx, loaded: Vec::new() }
    }

    /// Asks for a system font that has `c`. Each character is looked up once.
    pub fn want(&mut self, c: char) {
        if self.asked.insert(c) {
            let _ = self.tx.send(c);
        }
    }

    /// Asks for the characters of `text` that the interface font can't draw (tab and pane titles).
    pub fn want_text(&mut self, ctx: &egui::Context, text: &str) {
        if text.is_ascii() {
            return;
        }
        let font = egui::FontId::proportional(14.0);
        let missing: Vec<char> = ctx.fonts(|f| text.chars().filter(|&c| !c.is_ascii() && c != TOFU && !f.has_glyph(&font, c)).collect());
        missing.into_iter().for_each(|c| self.want(c));
    }

    /// Takes in the fonts found since the last call. True means `install` has to run again.
    pub fn poll(&mut self) -> bool {
        let before = self.loaded.len();
        self.loaded.extend(self.rx.try_iter());
        self.loaded.len() > before
    }
}

pub struct Installed {
    /// True when there is no separate bold face and bold must be faked.
    pub faux_bold: bool,
}

/// Installs fonts, preferring `preferred` (a family name; empty = the bundled JetBrains Mono).
/// The system fonts in `fallbacks` go behind them in every family.
pub fn install(ctx: &egui::Context, preferred: &str, fallbacks: &Fallbacks) -> Installed {
    let mut defs = FontDefinitions::default();
    // Bundled Hack + emoji fonts stay as fallbacks for missing glyphs.
    let fallback = defs.families[&FontFamily::Monospace].clone();
    defs.font_data.insert("st-jb".into(), FontData::from_static(JB_REGULAR));
    defs.font_data.insert("st-jb-bold".into(), FontData::from_static(JB_BOLD));
    defs.font_data.insert("st-inter".into(), FontData::from_static(INTER));

    let wanted = std::env::var("S_TERM_FONT").ok().filter(|f| !f.is_empty()).unwrap_or_else(|| preferred.to_string());
    let mut mono = Vec::new();
    let mut bold = Vec::new();
    let mut faux_bold = false;
    if !wanted.is_empty() && wanted != BUNDLED_MONO {
        if let Some((reg_file, regular)) = find(&wanted, "Regular") {
            defs.font_data.insert("st-regular".into(), regular);
            mono.push("st-regular".to_string());
            match find(&wanted, "Bold").filter(|(f, _)| *f != reg_file) {
                Some((_, data)) => {
                    defs.font_data.insert("st-bold".into(), data);
                    bold.push("st-bold".to_string());
                }
                None => faux_bold = true,
            }
        }
    }
    if mono.is_empty() {
        mono.push("st-jb".into());
        bold.push("st-jb-bold".into());
    }
    // Fallbacks: JetBrains Mono, then Hack + emoji.
    mono.push("st-jb".into());
    mono.extend(fallback.iter().cloned());
    mono.dedup();
    // System fonts go ahead of egui's emoji fonts, so that a symbol the emoji font also has (✳) comes
    // out the same size as its neighbours that it lacks (✢ ✶ ✻ ✽).
    let emoji = mono.iter().position(|name| name.to_lowercase().contains("emoji")).unwrap_or(mono.len());
    for (i, data) in fallbacks.loaded.iter().enumerate() {
        defs.font_data.insert(format!("st-system-{i}"), data.clone());
        mono.insert(emoji + i, format!("st-system-{i}"));
    }
    bold.extend(mono.iter().cloned());
    bold.dedup();
    defs.families.insert(FontFamily::Monospace, mono.clone());
    defs.families.insert(FontFamily::Name(MONO_BOLD.into()), bold);

    // Interface: Inter, with egui's defaults and the coding font behind it for symbols (✓ ↑ ⏎ …).
    let mut ui = vec!["st-inter".to_string()];
    ui.extend(defs.families[&FontFamily::Proportional].iter().cloned());
    ui.extend(mono);
    ui.dedup();
    defs.families.insert(FontFamily::Proportional, ui);
    ctx.set_fonts(defs);
    Installed { faux_bold }
}

/// Installed monospace font families (for the settings picker).
pub fn monospace_families() -> Vec<String> {
    let Ok(out) = Command::new("fc-list").args([":spacing=mono", "family"]).output() else { return Vec::new() };
    let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    v.sort_by_key(|s| s.to_lowercase());
    v.dedup();
    v
}

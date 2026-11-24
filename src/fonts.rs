//! Fonts. S-term ships JetBrains Mono (terminal) and Inter (interface), both under the SIL Open
//! Font License (see `assets/fonts/`). Any installed monospace font can be picked in Settings,
//! or forced with `S_TERM_FONT="Family Name"`.

use std::process::Command;

use egui::{FontData, FontDefinitions, FontFamily};

pub const MONO_BOLD: &str = "s-term-mono-bold";
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
    let mut data = FontData::from_owned(std::fs::read(&file).ok()?);
    data.index = index;
    Some((file, data))
}

pub struct Installed {
    /// True when there is no separate bold face and bold must be faked.
    pub faux_bold: bool,
}

/// Installs fonts, preferring `preferred` (a family name; empty = the bundled JetBrains Mono).
pub fn install(ctx: &egui::Context, preferred: &str) -> Installed {
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

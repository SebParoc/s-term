//! Settings page (Ctrl+,): every change applies live and is saved to the config file.

use std::path::Path;
use std::sync::Arc;

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Rect, RichText, Sense, Stroke};

use super::chrome::TITLE_H;
use super::STerm;
use crate::config::{Config, CursorStyle, HeatMetric, PaneHeaders, PromptPrefix};
use crate::icons::{self, Icon};
use crate::sidebar::Tab as View;
use crate::theme::{c32, lerp_color, Theme, PRESETS};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Section {
    #[default]
    Appearance,
    Terminal,
    Explorer,
    Assistant,
    Ai,
    Keys,
    About,
}

#[derive(Default)]
pub struct SettingsUi {
    section: Section,
    fonts: Option<Vec<String>>,
    /// "Add folder" field in the Explorer section.
    new_folder: String,
    /// About → Reset settings was clicked once and asks to confirm.
    confirm_reset: bool,
}

const SECTIONS: [(Section, Icon, &str); 7] = [
    (Section::Appearance, Icon::Palette, "Appearance"),
    (Section::Terminal, Icon::Terminal, "Terminal"),
    (Section::Explorer, Icon::Files, "Explorer"),
    (Section::Assistant, Icon::Sparkle, "Assistant"),
    (Section::Ai, Icon::Panel, "AI tools"),
    (Section::Keys, Icon::Keyboard, "Keybindings"),
    (Section::About, Icon::Info, "About"),
];

/// Every setting, for the command palette: (section id for `settings_section_by_name`, section
/// title, setting title, extra search words).
pub(super) const SETTINGS_INDEX: &[(&str, &str, &str, &str)] = &[
    ("appearance", "Appearance", "Theme", "color scheme dark light"),
    ("appearance", "Appearance", "Font family", "typeface monospace"),
    ("appearance", "Appearance", "Font size", "text zoom"),
    ("appearance", "Appearance", "Zoom scales", "ctrl wheel"),
    ("appearance", "Appearance", "Interface scale", "ui size dpi"),
    ("appearance", "Appearance", "Line height", "spacing"),
    ("appearance", "Appearance", "Animations", "motion"),
    ("appearance", "Appearance", "Cursor glide", "trail"),
    ("appearance", "Appearance", "Cursor blink", ""),
    ("appearance", "Appearance", "Pane title bars", "headers"),
    ("appearance", "Appearance", "Dim inactive panes", "focus"),
    ("appearance", "Appearance", "Status bar", "bottom"),
    ("appearance", "Appearance", "System title bar", "decorations window"),
    ("appearance", "Appearance", "Minimize and maximize buttons", "window"),
    ("terminal", "Terminal", "Shell command", "bash zsh fish program"),
    ("terminal", "Terminal", "Scrollback", "history lines"),
    ("terminal", "Terminal", "New tabs open in", "working directory folder cwd"),
    ("terminal", "Terminal", "Shell prompt", "before the ❯ user host folder"),
    ("terminal", "Terminal", "Cursor style", "bar block underline"),
    ("terminal", "Terminal", "Scroll speed", "mouse wheel"),
    ("terminal", "Terminal", "Copy on select", "clipboard"),
    ("terminal", "Terminal", "Remember closed tabs", "history reopen restore"),
    ("terminal", "Terminal", "Reopen last session at startup", "history restore session tabs"),
    ("explorer", "Explorer", "Explorer folders", "add folder files"),
    ("explorer", "Explorer", "Show hidden files", "dotfiles"),
    ("explorer", "Explorer", "Hide ignored files", "gitignore"),
    ("explorer", "Explorer", "Open panel at startup", "sidebar"),
    ("assistant", "Assistant", "Provider and key", "api key anthropic openai"),
    ("assistant", "Assistant", "Model", "assistant ai"),
    ("assistant", "Assistant", "Run commands without asking", "auto run approve"),
    ("assistant", "Assistant", "Privacy", ""),
    ("ai", "AI tools", "Claude Code", "usage tokens"),
    ("ai", "AI tools", "Codex CLI", "usage tokens"),
    ("ai", "AI tools", "Antigravity CLI", "usage tokens"),
    ("ai", "AI tools", "Activity heatmap", "usage"),
    ("keys", "Keybindings", "Keybindings", "shortcuts keyboard"),
    ("about", "About", "Version", "about s-term build"),
    ("about", "About", "What's inside", "features tour overview"),
    ("about", "About", "Your data", "config folder files json keys history privacy path"),
    ("about", "About", "Run setup", "first run onboarding welcome"),
    ("about", "About", "Reset settings", "defaults"),
    ("about", "About", "Credits", "license fonts alacritty egui"),
];

/// Where a tile on the About page leads.
#[derive(Clone, Copy)]
enum Go {
    View(View),
    Ask,
    Palette,
    Section(Section),
}

/// One line on the About page's hero card.
const TAGLINE: &str = "A modern terminal: splits and tabs, your repo at a glance, one-click launchers and a history of \
                       everything you closed. AI usage and an assistant are there if you want them.";

/// The About page's feature tiles: icon (None = the S-term glyph), name, what it does, shortcut, where a click goes.
const TOUR: [(Option<Icon>, &str, &str, &str, Go); 8] = [
    (Some(Icon::SplitRight), "Splits and tabs", "Split any pane, maximize one, or type into every pane of a tab at once.", "Ctrl+Shift+E", Go::Section(Section::Keys)),
    (Some(Icon::Search), "Command palette", "Every action, view, theme and setting, a few keystrokes away.", "Ctrl+Shift+P", Go::Palette),
    (Some(Icon::Files), "Explorer", "The focused pane's project tree, with a file, Markdown and image viewer.", "", Go::View(View::Files)),
    (Some(Icon::Git), "Source control", "Changed files with their diffs, and the branch's recent commits.", "", Go::View(View::Changes)),
    (Some(Icon::Play), "Launchpad", "Save the commands that start a project, then run them in one click.", "", Go::View(View::Launch)),
    (Some(Icon::History), "History", "Reopen closed tabs and your last session, with their folders and splits.", "Ctrl+Shift+H", Go::View(View::History)),
    (None, "Ask S-term", "Optional: an assistant that reads your panes and runs commands once you say OK.", "", Go::Ask),
    (Some(Icon::Sparkle), "AI usage", "Optional: context, plan limits and tokens for Claude Code, Codex and Antigravity.", "", Go::View(View::Ai)),
];

const CREDITS: &[(&str, &str)] = &[
    ("alacritty_terminal", "Terminal emulation, scrollback and the PTY"),
    ("egui and eframe", "The window and everything drawn in it"),
    ("JetBrains Mono", "The terminal font (SIL Open Font License)"),
    ("Inter", "The interface font (SIL Open Font License)"),
];

/// A feature tile on the About page: icon, name, optional shortcut chip and a wrapped description.
#[allow(clippy::too_many_arguments)]
fn tile(ui: &mut egui::Ui, theme: &Theme, size: egui::Vec2, icon: Option<Icon>, logo: egui::TextureId, title: &str, desc: Arc<egui::Galley>, keys: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(size, crate::app::CLICK);
    let hv = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    p.rect_filled(r, 12.0, lerp_color(theme.elevated, theme.header_focused, 0.6 * hv));
    p.rect_stroke(r.shrink(0.5), 12.0, Stroke::new(1.0_f32, lerp_color(theme.border, theme.accent.gamma_multiply(0.6), hv)));
    let c = pos2(r.min.x + 27.0, r.min.y + 27.0);
    p.circle_filled(c, 15.0, theme.accent.gamma_multiply(0.14 + 0.08 * hv));
    match icon {
        Some(i) => icons::draw(p, i, icons::center_rect(c, 15.0), theme.accent),
        None => super::chrome::paint_glyph(p, logo, Rect::from_center_size(c, vec2(19.0, 19.0)), theme.accent),
    }
    let x = r.min.x + 52.0;
    p.text(pos2(x, r.min.y + 20.0), Align2::LEFT_CENTER, title, FontId::proportional(13.5), theme.text);
    if !keys.is_empty() {
        let g = p.layout_no_wrap(keys.to_string(), FontId::monospace(10.5), theme.muted);
        let kr = Rect::from_min_size(pos2(r.max.x - 24.0 - g.size().x, r.min.y + 11.0), g.size() + vec2(12.0, 6.0));
        p.rect_filled(kr, 5.0, theme.bar);
        p.rect_stroke(kr.shrink(0.5), 5.0, Stroke::new(1.0_f32, theme.border));
        p.galley(kr.min + vec2(6.0, 3.0), g, theme.muted);
    }
    p.galley(pos2(x, r.min.y + 34.0), desc, theme.muted);
    resp
}

/// Opens a file or folder in the desktop's default app.
fn open_external(p: &Path) {
    match std::process::Command::new("xdg-open").arg(p).spawn() {
        // Reap it when it exits so it doesn't linger as a zombie.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => eprintln!("s-term: xdg-open {}: {e}", p.display()),
    }
}

const KEYS: &[(&str, &str)] = &[
    ("Ctrl+Shift+P", "Command palette"),
    ("Ctrl+,", "Settings"),
    ("Ctrl+Shift+T", "New tab"),
    ("Ctrl+Shift+E", "Split right"),
    ("Ctrl+Shift+O", "Split down"),
    ("Ctrl+Shift+W", "Close pane"),
    ("Ctrl+Shift+R", "Reopen closed tab"),
    ("Ctrl+Shift+H", "History"),
    ("Ctrl+Shift+X", "Maximize / restore pane"),
    ("Alt+Arrows", "Move focus between panes"),
    ("Ctrl+Shift+Arrows", "Resize pane"),
    ("Ctrl+Shift+N", "New window"),
    ("Ctrl+PgUp / PgDn, Ctrl+Tab", "Previous / next tab"),
    ("Ctrl+Shift+B", "Toggle side panel"),
    ("Alt+A / Alt+O", "Broadcast input on / off"),
    ("Ctrl+Shift+C / V", "Copy / paste"),
    ("Shift+PgUp / PgDn / Home / End", "Scroll back"),
    ("Ctrl+= / Ctrl+- / Ctrl+0", "Font bigger / smaller / reset"),
    ("Shift+Enter", "Newline without submitting (Claude Code)"),
    ("Ctrl+Shift+Q", "Quit"),
];

/// iOS-style switch.
pub(super) fn switch(ui: &mut egui::Ui, theme: &Theme, on: &mut bool, id: Id) -> egui::Response {
    let (r, mut resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(id, *on, 0.15);
    let p = ui.painter();
    // A near-white accent would swallow the white knob, so sink the track toward the background.
    let a = theme.accent;
    let luma = (0.2126 * a.r() as f32 + 0.7152 * a.g() as f32 + 0.0722 * a.b() as f32) / 255.0;
    let track = if luma > 0.75 { lerp_color(a, c32(theme.bg), 0.55) } else { a };
    p.rect_filled(r, 11.0, lerp_color(theme.header, track, t));
    p.rect_stroke(r.shrink(0.5), 11.0, Stroke::new(1.0_f32, lerp_color(theme.border, theme.accent, t)));
    let x = egui::lerp(r.min.x + 11.0..=r.max.x - 11.0, t);
    p.circle_filled(pos2(x, r.center().y), 8.0, lerp_color(theme.muted, Color32::WHITE, t));
    resp
}

/// Mutually exclusive options in one pill (non-focusable, so it can sit in the main window too).
pub(crate) fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, theme: &Theme, value: &mut T, opts: &[(T, &str)], id: &str) {
    let font = FontId::proportional(12.0);
    let widths: Vec<f32> = opts.iter().map(|(_, l)| ui.painter().layout_no_wrap(l.to_string(), font.clone(), Color32::WHITE).size().x + 22.0).collect();
    let (r, _) = ui.allocate_exact_size(vec2(widths.iter().sum::<f32>() + 4.0, 28.0), Sense::hover());
    ui.painter().rect_filled(r, 8.0, theme.bar);
    ui.painter().rect_stroke(r.shrink(0.5), 8.0, Stroke::new(1.0_f32, theme.border));
    let slots: Vec<Rect> = {
        let mut x = r.min.x + 2.0;
        widths.iter().map(|w| { let s = Rect::from_min_size(pos2(x, r.min.y + 2.0), vec2(*w, r.height() - 4.0)); x += w; s }).collect()
    };
    if let Some(i) = opts.iter().position(|(v, _)| *value == *v) {
        let shown = crate::glass::slide_within(ui.ctx(), Id::new((id, "pick")), slots[i], r.min, true);
        crate::glass::pick(ui.painter(), shown, theme);
    }
    for (i, ((v, label), br)) in opts.iter().zip(slots).enumerate() {
        let resp = ui.interact(br, Id::new((id, i)), crate::app::CLICK);
        let on = ui.ctx().animate_bool_with_time(Id::new((id, i, "on")), *value == *v, 0.15);
        let col = lerp_color(lerp_color(theme.muted, theme.text, if resp.hovered() { 0.5 } else { 0.0 }), theme.pick_ink, on);
        ui.painter().text(br.center(), Align2::CENTER_CENTER, *label, font.clone(), col);
        if resp.clicked() {
            *value = *v;
        }
    }
}

/// A settings row: title + description on the left, the control on the right.
pub(super) fn row(ui: &mut egui::Ui, theme: &Theme, title: &str, desc: &str, control: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width((ui.available_width() - 260.0).max(200.0));
            ui.label(RichText::new(title).size(13.5).color(theme.text));
            if !desc.is_empty() {
                ui.label(RichText::new(desc).size(11.5).color(theme.muted));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
    });
    ui.add_space(10.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, theme.border);
}

fn heading(ui: &mut egui::Ui, theme: &Theme, title: &str, sub: &str) {
    ui.label(RichText::new(title).size(22.0).strong().color(theme.text));
    ui.label(RichText::new(sub).size(12.5).color(theme.muted));
    ui.add_space(14.0);
}

fn group(ui: &mut egui::Ui, theme: &Theme, title: &str) {
    ui.add_space(18.0);
    ui.label(RichText::new(title.to_uppercase()).size(10.5).strong().color(theme.muted));
    ui.add_space(2.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, theme.border);
}

/// A clickable theme preview card.
fn theme_card(ui: &mut egui::Ui, current: &Theme, id: &'static str, name: &str, selected: bool) -> bool {
    let t = Theme::preset(id);
    let (r, resp) = ui.allocate_exact_size(vec2(158.0, 108.0), Sense::click());
    let hv = ui.ctx().animate_bool_with_time(Id::new(("s-term-themecard", id)), resp.hovered() || selected, 0.12);
    let p = ui.painter();
    let preview = Rect::from_min_size(r.min, vec2(r.width(), 78.0));
    p.rect_filled(preview, 10.0, c32(t.bg));
    // Fake title bar + panes.
    p.rect_filled(Rect::from_min_size(preview.min, vec2(preview.width(), 16.0)), egui::Rounding { nw: 10.0, ne: 10.0, sw: 0.0, se: 0.0 }, t.bar);
    p.rect_filled(Rect::from_min_size(preview.min + vec2(24.0, 4.0), vec2(40.0, 8.0)), 3.0, t.elevated);
    p.rect_filled(Rect::from_min_size(preview.min + vec2(28.0, 10.0), vec2(32.0, 1.5)), 1.0, t.accent);
    for (i, c) in t.palette[1..7].iter().enumerate() {
        p.circle_filled(preview.min + vec2(14.0 + i as f32 * 12.0, 30.0), 4.0, c32(*c));
    }
    for (j, w) in [70.0, 100.0, 54.0].iter().enumerate() {
        p.rect_filled(Rect::from_min_size(preview.min + vec2(10.0, 44.0 + j as f32 * 9.0), vec2(*w, 4.0)), 2.0, c32(t.fg).gamma_multiply(0.5));
    }
    p.rect_filled(Rect::from_min_size(preview.min + vec2(10.0 + 54.0 + 3.0, 62.0), vec2(6.0, 8.0)), 1.0, c32(t.cursor));
    let border = if selected { current.accent } else { lerp_color(current.border, current.accent.gamma_multiply(0.5), hv) };
    p.rect_stroke(preview.shrink(0.5), 10.0, Stroke::new(if selected { 2.0_f32 } else { 1.0 }, border));
    p.text(pos2(r.min.x + 4.0, r.max.y - 14.0), Align2::LEFT_CENTER, name, FontId::proportional(13.0), current.text);
    if selected {
        p.circle_filled(pos2(r.max.x - 12.0, r.max.y - 14.0), 8.0, current.accent);
        p.text(pos2(r.max.x - 12.0, r.max.y - 14.0), Align2::CENTER_CENTER, "✓", FontId::proportional(11.0), current.bar);
    }
    resp.clicked()
}

/// The shell snippet that reads the "folder in prompt" flags is installed: in ~/.bashrc.d, or
/// sourced from ~/.bashrc (install.sh gives that line when there is no ~/.bashrc.d).
fn prompt_snippet_installed() -> bool {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return false };
    let has = |p: std::path::PathBuf, needle: &str| std::fs::read_to_string(p).is_ok_and(|s| s.contains(needle));
    has(home.join(".bashrc.d/s-term-prompt.sh"), "prompt-user") || has(home.join(".bashrc"), "s-term-prompt.sh")
}

fn statusline_configured() -> bool {
    let p = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".claude/settings.json"));
    p.and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|s| s.contains("s-term statusline"))
}

impl STerm {
    pub(super) fn settings_section_by_name(&mut self, name: &str) {
        self.settings.section = match name {
            "terminal" => Section::Terminal,
            "explorer" => Section::Explorer,
            "ai" => Section::Ai,
            "assistant" => Section::Assistant,
            "keys" => Section::Keys,
            "about" => Section::About,
            _ => Section::Appearance,
        };
    }

    pub(super) fn settings_page(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let area = Rect::from_min_max(pos2(screen.min.x, screen.min.y + TITLE_H), screen.max);
        let mut close = false;

        egui::Area::new(Id::new("s-term-settings-backdrop")).order(egui::Order::Middle).fixed_pos(area.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(area.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(130));
            if resp.clicked() {
                close = true;
            }
        });

        let size = vec2((area.width() - 48.0).min(980.0), (area.height() - 48.0).min(760.0));
        // Centered in the whole window, nudged down only if it would run into the title bar.
        let mut rect = Rect::from_center_size(screen.center(), size);
        if rect.min.y < area.min.y + 16.0 {
            rect = rect.translate(vec2(0.0, area.min.y + 16.0 - rect.min.y));
        }
        let theme = Theme::preset(&self.config.theme);
        let mut cfg = self.config.clone();
        let mut reset = false;

        egui::Area::new(Id::new("s-term-settings")).order(egui::Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
            crate::glass::frame(ui, &theme, 22.0, theme.panel, crate::glass::Lift::High, 0.0, |ui| {
                    ui.set_min_size(size);
                    ui.set_max_size(size);
                    ui.horizontal_top(|ui| {
                        // No gap between the two columns, so the frame is exactly `size` and stays centered.
                        ui.spacing_mut().item_spacing.x = 0.0;
                        // ---- navigation
                        ui.vertical(|ui| {
                            ui.set_width(210.0);
                            ui.set_min_height(size.y);
                            let nav = Rect::from_min_size(ui.max_rect().min, vec2(210.0, size.y));
                            ui.painter().rect_filled(nav, egui::Rounding { nw: 22.0, sw: 22.0, ne: 0.0, se: 0.0 }, theme.bar);
                            ui.add_space(18.0);
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.label(RichText::new("Settings").size(17.0).strong().color(theme.text));
                            });
                            ui.add_space(14.0);
                            for (sec, icon, label) in SECTIONS {
                                let (r, resp) = ui.allocate_exact_size(vec2(210.0, 36.0), Sense::click());
                                let br = r.shrink2(vec2(10.0, 2.0));
                                let on = ui.ctx().animate_bool_with_time(Id::new(("s-term-set-nav", label)), self.settings.section == sec, 0.15);
                                let hv = if resp.hovered() { 0.5 } else { 0.0 };
                                ui.painter().rect_filled(br, 8.0, lerp_color(theme.header.gamma_multiply(hv), theme.header_focused, on));
                                icons::draw(ui.painter(), icon, icons::center_rect(pos2(br.min.x + 20.0, br.center().y), 15.0), lerp_color(theme.muted, theme.accent, on));
                                ui.painter().text(pos2(br.min.x + 38.0, br.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), lerp_color(theme.muted, theme.text, on.max(hv)));
                                if resp.clicked() {
                                    self.settings.section = sec;
                                    self.settings.confirm_reset = false;
                                }
                            }
                            ui.add_space(ui.available_height() - 40.0);
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.label(RichText::new("Saved automatically").size(10.5).color(theme.muted));
                            });
                        });

                        // ---- content
                        ui.vertical(|ui| {
                            ui.set_width(size.x - 210.0);
                            ui.horizontal(|ui| {
                                ui.add_space(ui.available_width() - 44.0);
                                if icons::button(ui, Icon::Close, 30.0, theme.muted, theme.header_focused, false, Id::new("s-term-set-close")).on_hover_text("Close (Esc)").clicked() {
                                    close = true;
                                }
                            });
                            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                                egui::Frame::none().inner_margin(egui::Margin { left: 28.0, right: 32.0, top: 0.0, bottom: 24.0 }).show(ui, |ui| {
                                    self.settings_section(ui, &theme, &mut cfg, &mut reset);
                                });
                            });
                        });
                    });
                });
        });

        if reset {
            // Launchers are the user's own work, not preferences: a reset keeps them.
            let launchers = std::mem::take(&mut cfg.launchers);
            cfg = Config { setup_done: true, launchers, ..Config::default() };
        }
        self.config = cfg;
        if close {
            self.settings_open = false;
        }
    }

    fn settings_section(&mut self, ui: &mut egui::Ui, theme: &Theme, cfg: &mut Config, reset: &mut bool) {
        match self.settings.section {
            Section::Appearance => {
                heading(ui, theme, "Appearance", "Theme, font and motion. Changes apply instantly.");
                group(ui, theme, "Theme");
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(14.0, 14.0);
                    for (id, name) in PRESETS {
                        if theme_card(ui, theme, id, name, cfg.theme == *id) {
                            cfg.theme = id.to_string();
                        }
                    }
                });
                group(ui, theme, "Font");
                let fonts = self.settings.fonts.get_or_insert_with(crate::fonts::monospace_families).clone();
                row(ui, theme, "Font family", "JetBrains Mono ships with S-term; any installed monospace font works too.", |ui| {
                    let label = if cfg.font_family.is_empty() { "JetBrains Mono (built-in)".to_string() } else { cfg.font_family.clone() };
                    egui::ComboBox::from_id_salt("s-term-font").selected_text(label).width(220.0).height(320.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut cfg.font_family, String::new(), "JetBrains Mono (built-in)");
                        for f in &fonts {
                            ui.selectable_value(&mut cfg.font_family, f.clone(), f);
                        }
                    });
                });
                row(ui, theme, "Font size", "Also Ctrl+= / Ctrl+- / Ctrl+0, or Ctrl + mouse wheel.", |ui| {
                    ui.add(egui::Slider::new(&mut cfg.font_size, 8.0..=32.0).step_by(0.5).suffix(" px"));
                });
                row(ui, theme, "Zoom scales", "What Ctrl+= / Ctrl+- / Ctrl+wheel make bigger or smaller.", |ui| {
                    segmented(ui, theme, &mut cfg.zoom_ui, &[(false, "Terminal text"), (true, "Whole interface")], "seg-zoom");
                });
                if cfg.zoom_ui {
                    row(ui, theme, "Interface scale", "Size of everything: tabs, panels, text.", |ui| {
                        ui.add(egui::Slider::new(&mut cfg.ui_scale, 0.6..=2.0).step_by(0.05).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                    });
                }
                row(ui, theme, "Line height", "Space between lines, as a multiple of the font's height.", |ui| {
                    ui.add(egui::Slider::new(&mut cfg.line_height, 0.9..=1.6).step_by(0.02).fixed_decimals(2));
                });
                group(ui, theme, "Motion");
                row(ui, theme, "Animations", "Panes glide when splitting and closing; tabs and focus fade.", |ui| {
                    switch(ui, theme, &mut cfg.animations, Id::new("sw-anim"));
                });
                row(ui, theme, "Cursor glide", "The cursor slides to its new position, leaving a short trail.", |ui| {
                    switch(ui, theme, &mut cfg.cursor_glide, Id::new("sw-glide"));
                });
                row(ui, theme, "Cursor blink", "The cursor gently fades in and out while you're idle.", |ui| {
                    switch(ui, theme, &mut cfg.cursor_blink, Id::new("sw-blink"));
                });
                group(ui, theme, "Layout");
                row(ui, theme, "Pane title bars", "Show a title bar on each pane.", |ui| {
                    segmented(ui, theme, &mut cfg.pane_headers, &[(PaneHeaders::Always, "Always"), (PaneHeaders::Splits, "With splits"), (PaneHeaders::Never, "Never")], "seg-headers");
                });
                row(ui, theme, "Dim inactive panes", "Darken panes that don't have focus.", |ui| {
                    switch(ui, theme, &mut cfg.dim_inactive, Id::new("sw-dim"));
                });
                row(ui, theme, "Status bar", "Branch, folder, AI usage and pane info at the bottom.", |ui| {
                    switch(ui, theme, &mut cfg.status_bar, Id::new("sw-status"));
                });
                row(ui, theme, "System title bar", "Use your desktop's window title bar instead of S-term's.", |ui| {
                    switch(ui, theme, &mut cfg.system_titlebar, Id::new("sw-titlebar"));
                });
                if !cfg.system_titlebar {
                    row(ui, theme, "Minimize and maximize buttons", "Show them next to the close button. Without them, double-click the title bar to maximize.", |ui| {
                        switch(ui, theme, &mut cfg.window_buttons, Id::new("sw-winbtns"));
                    });
                }
            }
            Section::Terminal => {
                heading(ui, theme, "Terminal", "Shell and behaviour. Shell and scrollback apply to new panes.");
                group(ui, theme, "Shell");
                row(ui, theme, "Shell command", "Program and arguments. Empty uses $SHELL.", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut cfg.shell).hint_text(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())).desired_width(220.0));
                });
                row(ui, theme, "Scrollback", "Lines kept in each pane's history.", |ui| {
                    ui.add(egui::DragValue::new(&mut cfg.scrollback).range(0..=1_000_000).speed(100.0).suffix(" lines"));
                });
                row(ui, theme, "New tabs open in", "Where a new tab's shell starts. Splits always open in the pane's folder.", |ui| {
                    segmented(ui, theme, &mut cfg.new_tab_home, &[(false, "Current folder"), (true, "Home")], "seg-newtab");
                });
                group(ui, theme, "Prompt");
                let snippet = prompt_snippet_installed();
                let user = format!(
                    "{}@{}",
                    std::env::var("USER").unwrap_or_else(|_| "user".into()),
                    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().split('.').next().unwrap_or("").to_string()).unwrap_or_else(|_| "host".into())
                );
                let example = match cfg.prompt {
                    PromptPrefix::None => "❯".to_string(),
                    PromptPrefix::Folder => "~/s-term ❯".to_string(),
                    PromptPrefix::HostFolder => format!("{user}:~/s-term ❯"),
                };
                row(ui, theme, "Before the ❯", &format!("What every pane's prompt shows: {example}. Applies at the next prompt."), |ui| {
                    segmented(
                        ui,
                        theme,
                        &mut cfg.prompt,
                        &[(PromptPrefix::None, "Nothing"), (PromptPrefix::Folder, "Folder"), (PromptPrefix::HostFolder, "Host + folder")],
                        "seg-prompt",
                    );
                });
                if !snippet {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Needs the S-term prompt script: run install.sh, which adds it to ~/.bashrc.d or gives you the line for ~/.bashrc.").size(11.5).color(c32(theme.palette[3])));
                }
                group(ui, theme, "Cursor & scrolling");
                row(ui, theme, "Cursor style", "\"Auto\" is a thin bar unless the running app asks for a shape (vim, for example).", |ui| {
                    segmented(
                        ui,
                        theme,
                        &mut cfg.cursor_style,
                        &[(CursorStyle::App, "Auto"), (CursorStyle::Block, "Block"), (CursorStyle::Beam, "Beam"), (CursorStyle::Underline, "Underline")],
                        "seg-cursor",
                    );
                });
                row(ui, theme, "Scroll speed", "How far the mouse wheel moves through scrollback.", |ui| {
                    ui.add(egui::Slider::new(&mut cfg.scroll_speed, 0.25..=5.0).step_by(0.25).custom_formatter(|v, _| format!("{v:.2}×")));
                });
                group(ui, theme, "Selection");
                row(ui, theme, "Copy on select", "Copy text to the clipboard as soon as you select it.", |ui| {
                    switch(ui, theme, &mut cfg.copy_on_select, Id::new("sw-cos"));
                });
                group(ui, theme, "History");
                row(ui, theme, "Remember closed tabs", "Closed tabs and panes, and the last session, go to the History view (Ctrl+Shift+H). Saves folders and command lines to ~/.local/share/s-term.", |ui| {
                    switch(ui, theme, &mut cfg.keep_history, Id::new("sw-history"));
                });
                ui.add_enabled_ui(cfg.keep_history, |ui| {
                    row(ui, theme, "Reopen last session at startup", "Off: a popup at the bottom offers it until you start typing.", |ui| {
                        switch(ui, theme, &mut cfg.restore_session, Id::new("sw-restore"));
                    });
                });
            }
            Section::Explorer => {
                heading(ui, theme, "Explorer", "The Files view. Extra folders are stacked under the focused pane's project.");
                group(ui, theme, "Folders");
                ui.add_space(8.0);
                ui.label(RichText::new("The focused pane's project is always shown first.").size(11.5).color(theme.muted));
                let mut remove = None;
                let mut swap = None;
                let n = cfg.explorer_folders.len();
                for (i, f) in cfg.explorer_folders.iter().enumerate() {
                    let exists = f.is_dir();
                    let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| f.to_string_lossy().into_owned());
                    let desc = if exists { f.to_string_lossy().into_owned() } else { format!("{} (not found)", f.to_string_lossy()) };
                    row(ui, theme, &name, &desc, |ui| {
                        if ui.button(RichText::new("Remove").color(c32(theme.palette[1]))).clicked() {
                            remove = Some(i);
                        }
                        if ui.add_enabled(i + 1 < n, egui::Button::new("↓")).on_hover_text("Move down").clicked() {
                            swap = Some((i, i + 1));
                        }
                        if ui.add_enabled(i > 0, egui::Button::new("↑")).on_hover_text("Move up").clicked() {
                            swap = Some((i - 1, i));
                        }
                    });
                }
                if let Some(i) = remove {
                    cfg.explorer_folders.remove(i);
                }
                if let Some((a, b)) = swap {
                    cfg.explorer_folders.swap(a, b);
                }
                let new = &mut self.settings.new_folder;
                let path = std::path::PathBuf::from(new.trim());
                let full = match (path.strip_prefix("~"), std::env::var_os("HOME")) {
                    (Ok(rest), Some(h)) => std::path::PathBuf::from(h).join(rest),
                    _ => path.clone(),
                };
                let valid = !new.trim().is_empty() && full.is_dir();
                let mut add = false;
                row(ui, theme, "Add folder", if new.trim().is_empty() || valid { "A path, or use + in the Explorer to browse." } else { "Not a folder." }, |ui| {
                    if ui.add_enabled(valid, egui::Button::new("Add")).clicked() {
                        add = true;
                    }
                    let r = ui.add(egui::TextEdit::singleline(new).hint_text("~/projects/other").desired_width(220.0));
                    if valid && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        add = true;
                    }
                });
                if add {
                    crate::sidebar::Sidebar::add_folder(cfg, path);
                    self.settings.new_folder.clear();
                }
                group(ui, theme, "Files");
                row(ui, theme, "Show hidden files", "List dotfiles such as .env and .github.", |ui| {
                    switch(ui, theme, &mut cfg.show_hidden, Id::new("sw-hidden"));
                });
                row(ui, theme, "Hide ignored files", "Leave out git-ignored files (target/, node_modules/) instead of dimming them.", |ui| {
                    switch(ui, theme, &mut cfg.hide_ignored, Id::new("sw-ignored"));
                });
                group(ui, theme, "Panel");
                row(ui, theme, "Open panel at startup", "Show the side panel when S-term starts.", |ui| {
                    switch(ui, theme, &mut cfg.panel_on_start, Id::new("sw-panelstart"));
                });
            }
            Section::Assistant => {
                heading(ui, theme, "Assistant", "Ask S-term from the box at the top of the window. It can read your panes, open new ones and run commands.");
                group(ui, theme, "Provider and key");
                row(ui, theme, "Provider", "Who answers your questions. A dot means a key is saved.", |ui| {
                    super::ask::provider_picker(ui, theme, cfg);
                });
                ui.add_space(10.0);
                ui.label(RichText::new("API key").size(13.5).color(theme.text));
                ui.label(RichText::new(super::ask::key_status(cfg.ai_provider)).size(11.5).color(theme.muted));
                ui.add_space(8.0);
                super::ask::key_field(ui, theme, cfg, &mut self.ask);
                ui.add_space(10.0);
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(r, 0.0, theme.border);
                group(ui, theme, "Model");
                let p = cfg.ai_provider;
                let has_key = crate::assistant::keys::get(p).is_some();
                if has_key {
                    self.ask.ensure_models(p, false);
                }
                let mut refresh = false;
                row(ui, theme, "Model", if has_key { "Loaded from your account. Automatic picks the newest." } else { "Add a key first." }, |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.spacing_mut().interact_size.y = 30.0;
                    if ui.add_enabled(has_key, egui::Button::new("Refresh").min_size(vec2(0.0, 30.0))).on_hover_text("Reload the model list").clicked() {
                        refresh = true;
                    }
                    let current = cfg.ai_model(p).to_string();
                    let label = if current.is_empty() { "Automatic".to_string() } else { crate::usage_ui::model_name(&current) };
                    ui.add_enabled_ui(has_key, |ui| {
                        egui::ComboBox::from_id_salt("s-term-set-model").selected_text(label).width(170.0).height(320.0).show_ui(ui, |ui| {
                            ui.selectable_value(cfg.ai_model_mut(p), String::new(), "Automatic");
                            let list = match self.ask.models.get(&p) {
                                Some(super::ask::ModelList::Ready(v)) => v.clone(),
                                Some(super::ask::ModelList::Loading(_)) => {
                                    ui.label(RichText::new("Loading…").color(theme.muted));
                                    Vec::new()
                                }
                                Some(super::ask::ModelList::Failed(e)) => {
                                    ui.label(RichText::new(e).color(c32(theme.palette[1])));
                                    Vec::new()
                                }
                                None => Vec::new(),
                            };
                            for id in list {
                                let name = crate::usage_ui::model_name(&id);
                                ui.selectable_value(cfg.ai_model_mut(p), id.clone(), name).on_hover_text(&id);
                            }
                        });
                    });
                });
                if refresh {
                    self.ask.ensure_models(p, true);
                }
                group(ui, theme, "Commands");
                row(
                    ui,
                    theme,
                    "Run commands without asking",
                    "Off: you approve every command, keystroke and closed pane first. On: S-term does them right away.",
                    |ui| {
                        switch(ui, theme, &mut cfg.ai_auto_run, Id::new("sw-autorun"));
                    },
                );
                group(ui, theme, "Privacy");
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "Nothing is sent until you ask. Each message goes to {} with a list of your panes and the focused pane's last 40 lines. \
                         The assistant reads other panes only through its tools, and you can see every tool it used in the conversation.",
                        p.name()
                    ))
                    .size(11.5)
                    .color(theme.muted),
                );
            }
            Section::Ai => {
                heading(ui, theme, "AI tools", "The AI coding tools S-term tracks. Turn off the ones you don't use.");
                group(ui, theme, "Tools");
                row(ui, theme, "Claude Code", "Tokens per model, context, plan limits.", |ui| {
                    switch(ui, theme, &mut cfg.show_claude, Id::new("sw-claude"));
                });
                row(ui, theme, "Codex CLI", "Tokens per model and the current session's context.", |ui| {
                    switch(ui, theme, &mut cfg.show_codex, Id::new("sw-codex"));
                });
                row(ui, theme, "Antigravity CLI", "Tokens per model and the current conversation.", |ui| {
                    switch(ui, theme, &mut cfg.show_antigravity, Id::new("sw-agy"));
                });
                row(ui, theme, "Activity heatmap", "What each day's square measures.", |ui| {
                    segmented(ui, theme, &mut cfg.heat_metric, &[(HeatMetric::Output, "Output tokens"), (HeatMetric::Total, "All tokens")], "seg-heat");
                });
                group(ui, theme, "Claude Code plan limits");
                let ok = statusline_configured();
                if !cfg.show_claude {
                    ui.add_space(8.0);
                    ui.label(RichText::new("Claude Code is turned off above.").size(11.5).color(theme.muted));
                    return;
                }
                row(
                    ui,
                    theme,
                    "Status-line hook",
                    "Claude Code shares its 5-hour / weekly limits only with its status line, which S-term reads.",
                    |ui| {
                        let (txt, col) = if ok { ("● Connected", c32(theme.palette[2])) } else { ("○ Not set up", c32(theme.palette[3])) };
                        ui.label(RichText::new(txt).color(col));
                    },
                );
                if !ok {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Add to ~/.claude/settings.json:").size(11.5).color(theme.muted));
                    ui.label(RichText::new(r#""statusLine": { "type": "command", "command": "~/.local/bin/s-term statusline" }"#).monospace().size(11.5).color(theme.accent2));
                }
            }
            Section::Keys => {
                heading(ui, theme, "Keybindings", "Terminator-style shortcuts, plus a command palette for everything else.");
                ui.add_space(6.0);
                for (k, what) in KEYS {
                    ui.horizontal(|ui| {
                        ui.set_min_height(30.0);
                        ui.label(RichText::new(*what).size(13.0).color(theme.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for part in k.split(", ").collect::<Vec<_>>().into_iter().rev() {
                                let g = ui.painter().layout_no_wrap(part.to_string(), FontId::monospace(11.5), theme.text);
                                let (r, _) = ui.allocate_exact_size(g.size() + vec2(14.0, 8.0), Sense::hover());
                                ui.painter().rect_filled(r, 6.0, theme.bar);
                                ui.painter().rect_stroke(r.shrink(0.5), 6.0, Stroke::new(1.0_f32, theme.border));
                                ui.painter().galley(r.min + vec2(7.0, 4.0), g, theme.text);
                            }
                        });
                    });
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                    ui.painter().rect_filled(r, 0.0, theme.border.gamma_multiply(0.6));
                }
            }
            Section::About => self.about_page(ui, theme, cfg, reset),
        }
    }

    fn about_page(&mut self, ui: &mut egui::Ui, theme: &Theme, cfg: &Config, reset: &mut bool) {
        heading(ui, theme, "About", "What S-term is, where it keeps your data, and how to start over.");
        let logo = self.logo.id();

        // ---- hero
        crate::usage_ui::card(ui, theme, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                ui.painter().rect_filled(r, 16.0, theme.bar);
                super::chrome::paint_logo(ui.painter(), logo, r.shrink(8.0), theme);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    ui.label(RichText::new("S-term").size(20.0).strong().color(theme.text));
                    ui.label(RichText::new(format!("Version {}  ·  Linux  ·  MIT license", env!("CARGO_PKG_VERSION"))).size(11.5).color(theme.muted));
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(RichText::new(TAGLINE).size(12.5).color(theme.text)).wrap());
                });
            });
        });

        // ---- what's inside: one tile per feature, click to open it
        group(ui, theme, "What's inside");
        ui.add_space(12.0);
        let gap = 10.0;
        let full = ui.available_width();
        let cols = if full >= 520.0 { 2 } else { 1 };
        let w = ((full - gap * (cols - 1) as f32) / cols as f32).floor();
        let mut go = None;
        for chunk in TOUR.chunks(cols) {
            let descs: Vec<_> = chunk.iter().map(|t| ui.painter().layout(t.2.to_string(), FontId::proportional(11.5), theme.muted, w - 66.0)).collect();
            let h = 48.0 + descs.iter().map(|g| g.size().y).fold(0.0, f32::max);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (&(icon, title, _, keys, to), desc) in chunk.iter().zip(descs) {
                    let tip = match to {
                        Go::Section(_) => "See all keybindings".to_string(),
                        Go::View(View::Ai) if !cfg.any_ai_tool() => "Pick the AI tools to track".to_string(),
                        _ => format!("Open {title}"),
                    };
                    if tile(ui, theme, vec2(w, h), icon, logo, title, desc, keys).on_hover_text(tip).clicked() {
                        go = Some(to);
                    }
                }
            });
            ui.add_space(gap - ui.spacing().item_spacing.y);
        }
        match go {
            Some(Go::View(View::Ai)) if !cfg.any_ai_tool() => self.settings.section = Section::Ai,
            Some(Go::View(v)) => {
                self.sidebar.show_view(v);
                self.settings_open = false;
            }
            Some(Go::Ask) if self.ask.has_key(cfg.ai_provider) || !self.ask.history.is_empty() || self.ask.busy() => {
                // Same as clicking the box in the title bar.
                self.settings_open = false;
                if self.ask.history.is_empty() && !self.ask.busy() {
                    self.palette.open_commands();
                    self.palette.ask_first = true;
                } else {
                    self.palette.open_ask();
                    self.ask.focus_input = true;
                }
            }
            // No key yet: the assistant needs one first.
            Some(Go::Ask) => self.settings.section = Section::Assistant,
            Some(Go::Palette) => {
                self.settings_open = false;
                self.palette.open_commands();
            }
            Some(Go::Section(s)) => self.settings.section = s,
            None => {}
        }

        // ---- your data
        group(ui, theme, "Your data");
        let config_dir = Config::path().parent().map(Path::to_path_buf).unwrap_or_default();
        let data_dir = crate::history::default_dir().unwrap_or_default();
        let places = [
            ("Settings and API keys", &config_dir, "Your settings and launchers (config.json), and API keys (keys.json, readable only by you)."),
            ("History and usage log", &data_dir, "Closed tabs and sessions (history.json), and daily token totals (usage-daily.json)."),
        ];
        for (title, dir, what) in places {
            row(ui, theme, title, &format!("{}  ·  {what}", crate::launchpad::tilde(dir)), |ui| {
                if ui.add_enabled(dir.is_dir(), egui::Button::new("Open folder")).on_disabled_hover_text("Nothing saved here yet").clicked() {
                    open_external(dir);
                }
            });
        }
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(
                RichText::new(
                    "S-term goes online only for the assistant, and only to the provider you picked. Usage tracking reads Claude Code, \
                     Codex and Antigravity files on this computer and never changes them.",
                )
                .size(11.5)
                .color(theme.muted),
            )
            .wrap(),
        );

        // ---- start over
        group(ui, theme, "Start over");
        row(ui, theme, "Run setup", "The first-run screen: which AI tools to track, and the assistant's provider and key.", |ui| {
            if ui.button("Run setup").clicked() {
                self.setup = Some(Default::default());
                self.settings_open = false;
            }
        });
        let confirm = &mut self.settings.confirm_reset;
        row(ui, theme, "Reset settings", "Every setting goes back to its default. Your launchers, API keys and History stay.", |ui| {
            if *confirm {
                // Right to left: reads "Reset everything?  Cancel  Reset".
                if ui.button(RichText::new("Reset").color(c32(theme.palette[1]))).clicked() {
                    *reset = true;
                    *confirm = false;
                }
                if ui.button("Cancel").clicked() {
                    *confirm = false;
                }
                ui.label(RichText::new("Reset everything?").size(12.0).color(theme.muted));
            } else if ui.button(RichText::new("Reset to defaults").color(c32(theme.palette[1]))).clicked() {
                *confirm = true;
            }
        });

        // ---- credits
        group(ui, theme, "Built with");
        ui.add_space(10.0);
        egui::Grid::new("s-term-about-credits").num_columns(2).spacing(vec2(24.0, 8.0)).show(ui, |ui| {
            for (name, what) in CREDITS {
                ui.label(RichText::new(*name).size(12.5).color(theme.text));
                ui.label(RichText::new(*what).size(12.0).color(theme.muted));
                ui.end_row();
            }
        });
    }
}

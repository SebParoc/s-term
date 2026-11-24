//! Settings page (Ctrl+,): every change applies live and is saved to the config file.

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Rect, RichText, Sense, Stroke};

use super::chrome::TITLE_H;
use super::STerm;
use crate::config::{Config, CursorStyle, HeatMetric, PaneHeaders, PromptPrefix};
use crate::icons::{self, Icon};
use crate::theme::{c32, lerp_color, Theme, PRESETS};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Section {
    #[default]
    Appearance,
    Terminal,
    Explorer,
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
}

const SECTIONS: [(Section, Icon, &str); 6] = [
    (Section::Appearance, Icon::Palette, "Appearance"),
    (Section::Terminal, Icon::Terminal, "Terminal"),
    (Section::Explorer, Icon::Files, "Explorer"),
    (Section::Ai, Icon::Sparkle, "AI & panel"),
    (Section::Keys, Icon::Keyboard, "Keybindings"),
    (Section::About, Icon::Info, "About"),
];

const KEYS: &[(&str, &str)] = &[
    ("Ctrl+Shift+P", "Command palette"),
    ("Ctrl+,", "Settings"),
    ("Ctrl+Shift+T", "New tab"),
    ("Ctrl+Shift+E", "Split right"),
    ("Ctrl+Shift+O", "Split down"),
    ("Ctrl+Shift+W", "Close pane"),
    ("Ctrl+Shift+X", "Maximize / restore pane"),
    ("Alt+Arrows", "Move focus between panes"),
    ("Ctrl+Shift+Arrows", "Resize pane"),
    ("Ctrl+Shift+N", "Next pane"),
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
fn switch(ui: &mut egui::Ui, theme: &Theme, on: &mut bool, id: Id) -> egui::Response {
    let (r, mut resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(id, *on, 0.15);
    let p = ui.painter();
    p.rect_filled(r, 11.0, lerp_color(theme.header, theme.accent, t));
    p.rect_stroke(r.shrink(0.5), 11.0, Stroke::new(1.0_f32, lerp_color(theme.border, theme.accent, t)));
    let x = egui::lerp(r.min.x + 11.0..=r.max.x - 11.0, t);
    p.circle_filled(pos2(x, r.center().y), 8.0, lerp_color(theme.muted, Color32::WHITE, t));
    resp
}

fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, theme: &Theme, value: &mut T, opts: &[(T, &str)], id: &str) {
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
        let shown = crate::glass::slide(ui.ctx(), Id::new((id, "pick")), slots[i], true);
        crate::glass::pick(ui.painter(), shown, theme);
    }
    for (i, ((v, label), br)) in opts.iter().zip(slots).enumerate() {
        let resp = ui.interact(br, Id::new((id, i)), Sense::click());
        let on = ui.ctx().animate_bool_with_time(Id::new((id, i, "on")), *value == *v, 0.15);
        let col = lerp_color(lerp_color(theme.muted, theme.text, if resp.hovered() { 0.5 } else { 0.0 }), theme.pick_ink, on);
        ui.painter().text(br.center(), Align2::CENTER_CENTER, *label, font.clone(), col);
        if resp.clicked() {
            *value = *v;
        }
    }
}

/// A settings row: title + description on the left, the control on the right.
fn row(ui: &mut egui::Ui, theme: &Theme, title: &str, desc: &str, control: impl FnOnce(&mut egui::Ui)) {
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

/// The shell snippet that reads the "folder in prompt" flag is installed.
fn prompt_snippet_installed() -> bool {
    let p = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".bashrc.d/s-term-prompt.sh"));
    p.and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|s| s.contains("prompt-user"))
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
            cfg = Config::default();
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
                row(ui, theme, "Cursor glide", "The cursor slides to its new position.", |ui| {
                    switch(ui, theme, &mut cfg.cursor_glide, Id::new("sw-glide"));
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
                    ui.label(RichText::new("Needs the S-term prompt snippet: run scripts/install.sh, or copy scripts/s-term-prompt.sh to ~/.bashrc.d/.").size(11.5).color(c32(theme.palette[3])));
                }
                group(ui, theme, "Cursor & scrolling");
                row(ui, theme, "Cursor style", "\"Program\" uses the shape the running app asks for.", |ui| {
                    segmented(
                        ui,
                        theme,
                        &mut cfg.cursor_style,
                        &[(CursorStyle::App, "Program"), (CursorStyle::Block, "Block"), (CursorStyle::Beam, "Beam"), (CursorStyle::Underline, "Underline")],
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
            Section::Ai => {
                heading(ui, theme, "AI & panel", "What the side panel's AI view shows.");
                group(ui, theme, "Tools");
                row(ui, theme, "Claude Code", "Tokens per model, context, plan limits.", |ui| {
                    switch(ui, theme, &mut cfg.show_claude, Id::new("sw-claude"));
                });
                row(ui, theme, "Antigravity CLI", "Tokens per model and the current conversation.", |ui| {
                    switch(ui, theme, &mut cfg.show_antigravity, Id::new("sw-agy"));
                });
                row(ui, theme, "Activity heatmap", "What each day's square measures.", |ui| {
                    segmented(ui, theme, &mut cfg.heat_metric, &[(HeatMetric::Output, "Output tokens"), (HeatMetric::Total, "All tokens")], "seg-heat");
                });
                group(ui, theme, "Claude Code plan limits");
                let ok = statusline_configured();
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
            Section::About => {
                heading(ui, theme, "About S-term", "A terminal for vibe coding, written in Rust.");
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                    ui.painter().rect_filled(r, 14.0, theme.bar);
                    super::chrome::paint_logo(ui.painter(), self.logo.id(), r.shrink(6.0), theme);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("S-term").size(18.0).strong().color(theme.text));
                        ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(theme.muted));
                    });
                });
                group(ui, theme, "Files");
                let cfg_path = Config::path();
                row(ui, theme, "Config file", &cfg_path.to_string_lossy(), |ui| {
                    if ui.button("Open").clicked() {
                        let _ = std::process::Command::new("xdg-open").arg(&cfg_path).spawn();
                    }
                });
                row(ui, theme, "Reset settings", "Go back to the defaults.", |ui| {
                    if ui.button(RichText::new("Reset to defaults").color(c32(theme.palette[1]))).clicked() {
                        *reset = true;
                    }
                });
            }
        }
    }
}

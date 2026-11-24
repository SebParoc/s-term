//! Command palette (Ctrl+Shift+P): fuzzy-search every action, view, theme and toggle.

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Key, Modifiers, Rect, RichText, Sense, Stroke};

use super::STerm;
use crate::config::{PaneHeaders, PromptPrefix};
use crate::icons::{self, Icon};
use crate::input::{Action, Dir};
use crate::sidebar::Tab as View;
use crate::theme::{lerp_color, PRESETS};

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    query: String,
    selected: usize,
    just_opened: bool,
}

impl Palette {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.query.clear();
            self.selected = 0;
            self.just_opened = true;
        }
    }
}

#[derive(Clone)]
enum Cmd {
    Act(Action),
    View(View),
    Settings,
    Theme(&'static str),
    ToggleAnimations,
    ToggleStatusBar,
    ToggleTitlebar,
    ToggleCursorGlide,
    CycleHeaders,
    ToggleDim,
    ClearScrollback,
    AddFolder,
    CyclePrompt,
    ToggleHidden,
}

struct Entry {
    label: String,
    group: &'static str,
    keys: &'static str,
    icon: Icon,
    cmd: Cmd,
}

/// Subsequence fuzzy match. Higher is better; None = no match.
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut ti = 0;
    let mut prev: Option<usize> = None;
    for qc in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = (ti..t.len()).find(|&i| t[i] == qc)?;
        score += 10;
        if prev.is_some_and(|p| p + 1 == found) {
            score += 15; // consecutive
        }
        if found == 0 || !t[found - 1].is_alphanumeric() {
            score += 20; // word start
        }
        score -= (found - ti) as i32; // gaps
        prev = Some(found);
        ti = found + 1;
    }
    Some(score - t.len() as i32 / 8)
}

fn entries(app: &STerm) -> Vec<Entry> {
    let e = |label: &str, group: &'static str, keys: &'static str, icon: Icon, cmd: Cmd| Entry { label: label.into(), group, keys, icon, cmd };
    let on_off = |b: bool| if b { "on → off" } else { "off → on" };
    let c = &app.config;
    let mut v = vec![
        e("New tab", "Terminal", "Ctrl+Shift+T", Icon::Plus, Cmd::Act(Action::NewTab)),
        e("New window", "Terminal", "Ctrl+Shift+N", Icon::Plus, Cmd::Act(Action::NewWindow)),
        e("Split right", "Terminal", "Ctrl+Shift+E", Icon::SplitRight, Cmd::Act(Action::SplitSideBySide)),
        e("Split down", "Terminal", "Ctrl+Shift+O", Icon::SplitDown, Cmd::Act(Action::SplitStacked)),
        e("Close pane", "Terminal", "Ctrl+Shift+W", Icon::Close, Cmd::Act(Action::ClosePane)),
        e("Maximize / restore pane", "Terminal", "Ctrl+Shift+X", Icon::Zoom, Cmd::Act(Action::ToggleZoom)),
        e("Next tab", "Terminal", "Ctrl+PgDn", Icon::ChevronRight, Cmd::Act(Action::NextTab)),
        e("Previous tab", "Terminal", "Ctrl+PgUp", Icon::ChevronRight, Cmd::Act(Action::PrevTab)),
        e("Next pane", "Terminal", "", Icon::ChevronRight, Cmd::Act(Action::NextPane)),
        e("Previous pane", "Terminal", "", Icon::ChevronRight, Cmd::Act(Action::PrevPane)),
        e("Focus pane left", "Terminal", "Alt+←", Icon::ChevronRight, Cmd::Act(Action::Focus(Dir::Left))),
        e("Focus pane right", "Terminal", "Alt+→", Icon::ChevronRight, Cmd::Act(Action::Focus(Dir::Right))),
        e("Broadcast input to all panes", "Terminal", "Alt+A", Icon::Broadcast, Cmd::Act(Action::BroadcastAll)),
        e("Stop broadcasting", "Terminal", "Alt+O", Icon::Broadcast, Cmd::Act(Action::BroadcastOff)),
        e("Clear scrollback", "Terminal", "", Icon::Text, Cmd::ClearScrollback),
        e("Scroll to top", "Terminal", "Shift+Home", Icon::ChevronRight, Cmd::Act(Action::ScrollTop)),
        e("Scroll to bottom", "Terminal", "Shift+End", Icon::ChevronRight, Cmd::Act(Action::ScrollBottom)),
        e("Show Explorer", "View", "", Icon::Files, Cmd::View(View::Files)),
        e("Explorer: add folder…", "View", "", Icon::Folder, Cmd::AddFolder),
        e(&format!("Explorer: toggle hidden files ({})", on_off(c.show_hidden)), "View", "", Icon::Files, Cmd::ToggleHidden),
        e("Show Source Control", "View", "", Icon::Git, Cmd::View(View::Changes)),
        e("Show AI Usage", "View", "", Icon::Sparkle, Cmd::View(View::Ai)),
        e("Toggle side panel", "View", "Ctrl+Shift+B", Icon::Panel, Cmd::Act(Action::ToggleSidebar)),
        e("Open settings", "Preferences", "Ctrl+,", Icon::Gear, Cmd::Settings),
        e("Font: bigger", "Preferences", "Ctrl+=", Icon::Text, Cmd::Act(Action::FontBigger)),
        e("Font: smaller", "Preferences", "Ctrl+-", Icon::Text, Cmd::Act(Action::FontSmaller)),
        e("Font: reset size", "Preferences", "Ctrl+0", Icon::Text, Cmd::Act(Action::FontReset)),
        e(&format!("Toggle animations ({})", on_off(c.animations)), "Preferences", "", Icon::Sparkle, Cmd::ToggleAnimations),
        e(&format!("Toggle cursor glide ({})", on_off(c.cursor_glide)), "Preferences", "", Icon::Text, Cmd::ToggleCursorGlide),
        e(&format!("Toggle status bar ({})", on_off(c.status_bar)), "Preferences", "", Icon::Panel, Cmd::ToggleStatusBar),
        e(&format!("Toggle system title bar ({})", on_off(c.system_titlebar)), "Preferences", "", Icon::Maximize, Cmd::ToggleTitlebar),
        e(&format!("Toggle dim inactive panes ({})", on_off(c.dim_inactive)), "Preferences", "", Icon::SplitRight, Cmd::ToggleDim),
        e("Cycle shell prompt (arrow / folder / user@host + folder)", "Preferences", "", Icon::Terminal, Cmd::CyclePrompt),
        e("Cycle pane title bars (always / splits / never)", "Preferences", "", Icon::Terminal, Cmd::CycleHeaders),
        e("Quit S-term", "Application", "Ctrl+Shift+Q", Icon::Close, Cmd::Act(Action::Quit)),
    ];
    for (id, name) in PRESETS {
        let cur = if *id == app.config.theme { "  ✓" } else { "" };
        v.push(e(&format!("Theme: {name}{cur}"), "Theme", "", Icon::Palette, Cmd::Theme(id)));
    }
    v
}

impl STerm {
    fn run_cmd(&mut self, cmd: Cmd, ctx: &egui::Context) {
        match cmd {
            Cmd::Act(a) => self.do_action(a, ctx),
            Cmd::View(v) => self.sidebar.show_view(v),
            Cmd::Settings => self.settings_open = true,
            Cmd::Theme(t) => self.config.theme = t.to_string(),
            Cmd::ToggleAnimations => self.config.animations ^= true,
            Cmd::ToggleStatusBar => self.config.status_bar ^= true,
            Cmd::ToggleTitlebar => self.config.system_titlebar ^= true,
            Cmd::ToggleCursorGlide => self.config.cursor_glide ^= true,
            Cmd::ToggleDim => self.config.dim_inactive ^= true,
            Cmd::CycleHeaders => {
                self.config.pane_headers = match self.config.pane_headers {
                    PaneHeaders::Always => PaneHeaders::Splits,
                    PaneHeaders::Splits => PaneHeaders::Never,
                    PaneHeaders::Never => PaneHeaders::Always,
                }
            }
            Cmd::ClearScrollback => self.clear_scrollback(),
            Cmd::AddFolder => self.sidebar.start_adding(),
            Cmd::CyclePrompt => {
                self.config.prompt = match self.config.prompt {
                    PromptPrefix::None => PromptPrefix::Folder,
                    PromptPrefix::Folder => PromptPrefix::HostFolder,
                    PromptPrefix::HostFolder => PromptPrefix::None,
                }
            }
            Cmd::ToggleHidden => self.config.show_hidden ^= true,
        }
    }

    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        let all = entries(self);
        let mut list: Vec<(i32, &Entry)> = all
            .iter()
            .filter_map(|e| fuzzy(&self.palette.query, &format!("{} {}", e.label, e.group)).map(|s| (s, e)))
            .collect();
        list.sort_by(|a, b| b.0.cmp(&a.0));
        let n = list.len();
        if n > 0 {
            self.palette.selected = self.palette.selected.min(n - 1);
        }
        let (down, up, enter) = ctx.input_mut(|i| {
            (i.consume_key(Modifiers::NONE, Key::ArrowDown), i.consume_key(Modifiers::NONE, Key::ArrowUp), i.consume_key(Modifiers::NONE, Key::Enter))
        });
        if n > 0 {
            if down {
                self.palette.selected = (self.palette.selected + 1) % n;
            }
            if up {
                self.palette.selected = (self.palette.selected + n - 1) % n;
            }
        }
        let mut run: Option<Cmd> = (enter && n > 0).then(|| list[self.palette.selected].1.cmd.clone());
        let theme = &self.theme;
        let screen = ctx.screen_rect();
        let width = (screen.width() - 40.0).min(620.0);
        let pos = pos2(screen.center().x - width / 2.0, screen.min.y + 64.0);
        let mut close = false;

        egui::Area::new(Id::new("s-term-palette-backdrop")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(90));
            if resp.clicked() {
                close = true;
            }
        });
        egui::Area::new(Id::new("s-term-palette")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            crate::glass::frame(ui, theme, 22.0, theme.elevated, crate::glass::Lift::High, 10.0, |ui| {
                    ui.set_width(width - 20.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(22.0, 28.0), Sense::hover());
                        icons::draw(ui.painter(), Icon::Search, icons::center_rect(r.center(), 15.0), theme.accent);
                        let edit = egui::TextEdit::singleline(&mut self.palette.query)
                            .hint_text("Type a command…")
                            .frame(false)
                            .font(FontId::proportional(15.0))
                            .desired_width(f32::INFINITY);
                        let resp = ui.add(edit);
                        if self.palette.just_opened {
                            resp.request_focus();
                            self.palette.just_opened = false;
                        }
                        if resp.changed() {
                            self.palette.selected = 0;
                        }
                    });
                    ui.add_space(4.0);
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
                        if list.is_empty() {
                            ui.add_space(8.0);
                            ui.label(RichText::new("No matching commands").color(theme.muted));
                        }
                        for (i, (_, e)) in list.iter().enumerate() {
                            let sel = i == self.palette.selected;
                            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                            if resp.hovered() && ui.input(|i| i.pointer.delta().length() > 0.0) {
                                self.palette.selected = i;
                            }
                            let t = ui.ctx().animate_bool_with_time(Id::new(("s-term-pal-sel", i)), sel, 0.08);
                            let p = ui.painter();
                            if t > 0.0 {
                                p.rect_filled(r, 8.0, theme.header_focused.gamma_multiply(t));
                            }
                            if sel {
                                ui.scroll_to_rect(r, None);
                            }
                            let ic = Rect::from_center_size(pos2(r.min.x + 20.0, r.center().y), vec2(26.0, 26.0));
                            p.rect_filled(ic, 7.0, theme.header);
                            icons::draw(p, e.icon, icons::center_rect(ic.center(), 14.0), lerp_color(theme.muted, theme.accent, t));
                            p.text(pos2(r.min.x + 42.0, r.center().y), Align2::LEFT_CENTER, &e.label, FontId::proportional(13.5), lerp_color(theme.text.gamma_multiply(0.85), theme.text, t));
                            let mut x = r.max.x - 10.0;
                            if !e.keys.is_empty() {
                                let g = p.layout_no_wrap(e.keys.to_string(), FontId::monospace(10.5), theme.muted);
                                let kr = Rect::from_min_size(pos2(x - g.size().x - 10.0, r.center().y - 10.0), vec2(g.size().x + 10.0, 20.0));
                                p.rect_filled(kr, 5.0, theme.bar);
                                p.rect_stroke(kr.shrink(0.5), 5.0, Stroke::new(1.0_f32, theme.border));
                                p.galley(pos2(kr.min.x + 5.0, kr.center().y - g.size().y / 2.0), g, theme.muted);
                                x = kr.min.x - 10.0;
                            }
                            p.text(pos2(x, r.center().y), Align2::RIGHT_CENTER, e.group, FontId::proportional(11.0), theme.muted.gamma_multiply(0.8));
                            if resp.clicked() {
                                run = Some(e.cmd.clone());
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("↑↓ navigate   ⏎ run   esc close").size(10.5).color(theme.muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(format!("{n} commands")).size(10.5).color(theme.muted));
                        });
                    });
                });
        });

        if let Some(cmd) = run {
            self.palette.open = false;
            self.run_cmd(cmd, ctx);
        } else if close {
            self.palette.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy;

    #[test]
    fn fuzzy_ranks_word_starts() {
        assert!(fuzzy("spr", "Split right").unwrap() > fuzzy("spr", "Show Source Control panel right").unwrap_or(i32::MIN));
        assert!(fuzzy("xyz", "New tab").is_none());
        assert!(fuzzy("", "anything").is_some());
        assert!(fuzzy("theme mid", "Theme: Midnight").is_some());
    }
}

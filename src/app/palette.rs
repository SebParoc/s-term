//! Command palette (Ctrl+Shift+P): fuzzy-search every action, view, theme and toggle.

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Key, Modifiers, Rect, RichText, Sense, Stroke};

use super::STerm;
use crate::config::{PaneHeaders, PromptPrefix};
use crate::icons::{self, Icon};
use crate::input::{Action, Dir};
use crate::sidebar::Tab as View;
use crate::theme::{lerp_color, PRESETS};

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Fuzzy command search, with "Ask S-term" as one of the results.
    #[default]
    Commands,
    /// The conversation with the assistant.
    Ask,
}

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    pub mode: Mode,
    query: String,
    selected: usize,
    just_opened: bool,
    /// Opened from the "Ask S-term" box: asking is the first result for any query, and the
    /// commands sit in a dropdown under the text field.
    pub ask_first: bool,
    /// That dropdown is open.
    commands_open: bool,
    /// How far the Ask popup was dragged from its default spot.
    pub ask_offset: egui::Vec2,
    /// The Ask popup's size last frame, to keep it centered as it grows (and to size its window).
    ask_size: egui::Vec2,
    /// The Ask popup is its own OS window this frame (so it can leave the S-term window).
    pub ask_windowed: bool,
    /// It was on screen last frame (false = it opens fresh this frame).
    pub ask_shown: bool,
    /// Where its window opens: last place it was, else centered over S-term.
    ask_open_pos: Option<egui::Pos2>,
    ask_last_pos: Option<egui::Pos2>,
    /// Multiple OS windows don't work here; draw the popup inside the main window instead.
    ask_no_window: bool,
    /// Close the window for one frame so it reopens at `ask_open_pos`.
    ask_reopen: bool,
}

impl Palette {
    /// Ctrl+Shift+P: command search.
    pub fn toggle(&mut self) {
        if self.open && self.mode == Mode::Ask {
            self.open_commands();
            return;
        }
        self.open = !self.open;
        if self.open {
            self.open_commands();
        }
    }

    pub fn open_commands(&mut self) {
        self.open = true;
        self.mode = Mode::Commands;
        self.ask_first = false;
        self.commands_open = false;
        self.query.clear();
        self.selected = 0;
        self.just_opened = true;
    }

    /// Opens the commands dropdown (screenshots).
    pub fn show_commands(&mut self) {
        self.commands_open = true;
    }

    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_string();
    }

    /// Puts the Ask popup back in its default spot over S-term (it reopens there).
    pub fn recenter_ask(&mut self) {
        self.ask_offset = egui::Vec2::ZERO;
        self.ask_last_pos = None;
        if self.ask_windowed {
            // Reopen the window so it's placed afresh.
            self.ask_shown = false;
            self.ask_reopen = true;
        }
    }

    pub fn open_ask(&mut self) {
        self.open = true;
        self.mode = Mode::Ask;
    }
}

#[derive(Clone)]
enum Cmd {
    /// Send the query to the assistant.
    Ask,
    Act(Action),
    View(View),
    Settings,
    /// Open settings at a section (see `SETTINGS_INDEX`).
    SettingsAt(&'static str),
    Theme(&'static str),
    ToggleAnimations,
    ToggleStatusBar,
    ToggleTitlebar,
    ToggleCursorGlide,
    ToggleCursorBlink,
    CycleHeaders,
    ToggleDim,
    ClearScrollback,
    AddFolder,
    CyclePrompt,
    ToggleHidden,
    /// Run a Launchpad launcher (by id).
    Launch(u64),
    NewLauncher,
}

struct Entry {
    label: String,
    group: &'static str,
    keys: &'static str,
    icon: Icon,
    cmd: Cmd,
    /// Extra words the search matches but doesn't show.
    search: &'static str,
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
    let e = |label: &str, group: &'static str, keys: &'static str, icon: Icon, cmd: Cmd| Entry { label: label.into(), group, keys, icon, cmd, search: "" };
    let on_off = |b: bool| if b { "on → off" } else { "off → on" };
    let c = &app.config;
    let mut v = vec![
        e("New tab", "Terminal", "Ctrl+Shift+T", Icon::Plus, Cmd::Act(Action::NewTab)),
        e("New window", "Terminal", "Ctrl+Shift+N", Icon::Plus, Cmd::Act(Action::NewWindow)),
        e("Split right", "Terminal", "Ctrl+Shift+E", Icon::SplitRight, Cmd::Act(Action::SplitSideBySide)),
        e("Split down", "Terminal", "Ctrl+Shift+O", Icon::SplitDown, Cmd::Act(Action::SplitStacked)),
        e("Close pane", "Terminal", "Ctrl+Shift+W", Icon::Close, Cmd::Act(Action::ClosePane)),
        e("Reopen closed tab", "Terminal", "Ctrl+Shift+R", Icon::History, Cmd::Act(Action::ReopenClosed)),
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
        e("Show Launchpad", "View", "", Icon::Play, Cmd::View(View::Launch)),
        e("Launchpad: new launcher…", "Launchpad", "", Icon::Plus, Cmd::NewLauncher),
        e("Show History", "View", "Ctrl+Shift+H", Icon::History, Cmd::View(View::History)),
        e("Toggle side panel", "View", "Ctrl+Shift+B", Icon::Panel, Cmd::Act(Action::ToggleSidebar)),
        e("Open settings", "Preferences", "Ctrl+,", Icon::Gear, Cmd::Settings),
        e("Font: bigger", "Preferences", "Ctrl+=", Icon::Text, Cmd::Act(Action::FontBigger)),
        e("Font: smaller", "Preferences", "Ctrl+-", Icon::Text, Cmd::Act(Action::FontSmaller)),
        e("Font: reset size", "Preferences", "Ctrl+0", Icon::Text, Cmd::Act(Action::FontReset)),
        e(&format!("Toggle animations ({})", on_off(c.animations)), "Preferences", "", Icon::Sparkle, Cmd::ToggleAnimations),
        e(&format!("Toggle cursor glide ({})", on_off(c.cursor_glide)), "Preferences", "", Icon::Text, Cmd::ToggleCursorGlide),
        e(&format!("Toggle cursor blink ({})", on_off(c.cursor_blink)), "Preferences", "", Icon::Text, Cmd::ToggleCursorBlink),
        e(&format!("Toggle status bar ({})", on_off(c.status_bar)), "Preferences", "", Icon::Panel, Cmd::ToggleStatusBar),
        e(&format!("Toggle system title bar ({})", on_off(c.system_titlebar)), "Preferences", "", Icon::Maximize, Cmd::ToggleTitlebar),
        e(&format!("Toggle dim inactive panes ({})", on_off(c.dim_inactive)), "Preferences", "", Icon::SplitRight, Cmd::ToggleDim),
        e("Cycle shell prompt (arrow / folder / user@host + folder)", "Preferences", "", Icon::Terminal, Cmd::CyclePrompt),
        e("Cycle pane title bars (always / splits / never)", "Preferences", "", Icon::Terminal, Cmd::CycleHeaders),
        e("Quit S-term", "Application", "Ctrl+Shift+Q", Icon::Close, Cmd::Act(Action::Quit)),
    ];
    if c.any_ai_tool() {
        v.push(e("Show AI Usage", "View", "", Icon::Sparkle, Cmd::View(View::Ai)));
    }
    for l in &c.launchers {
        v.push(Entry { label: format!("Run: {}", l.name), group: "Launchpad", keys: "", icon: Icon::PlaySolid, cmd: Cmd::Launch(l.id), search: "launcher start" });
    }
    for (id, name) in PRESETS {
        let cur = if *id == app.config.theme { "  ✓" } else { "" };
        v.push(e(&format!("Theme: {name}{cur}"), "Theme", "", Icon::Palette, Cmd::Theme(id)));
    }
    for &(id, section, title, words) in super::settings::SETTINGS_INDEX {
        v.push(Entry { label: title.into(), group: section_group(section), keys: "", icon: Icon::Gear, cmd: Cmd::SettingsAt(id), search: words });
    }
    v
}

/// The right-hand label for a settings entry.
fn section_group(section: &str) -> &'static str {
    match section {
        "Appearance" => "Settings · Appearance",
        "Terminal" => "Settings · Terminal",
        "Explorer" => "Settings · Explorer",
        "Assistant" => "Settings · Assistant",
        "AI tools" => "Settings · AI tools",
        "Keybindings" => "Settings · Keybindings",
        "About" => "Settings · About",
        _ => "Settings",
    }
}

impl STerm {
    fn run_cmd(&mut self, cmd: Cmd, ctx: &egui::Context) {
        match cmd {
            Cmd::Ask => {}
            Cmd::Act(a) => self.do_action(a, ctx),
            Cmd::View(v) => self.sidebar.show_view(v),
            Cmd::Settings => self.settings_open = true,
            Cmd::SettingsAt(id) => {
                self.settings_section_by_name(id);
                self.settings_open = true;
            }
            Cmd::Theme(t) => self.config.theme = t.to_string(),
            Cmd::ToggleAnimations => self.config.animations ^= true,
            Cmd::ToggleStatusBar => self.config.status_bar ^= true,
            Cmd::ToggleTitlebar => self.config.system_titlebar ^= true,
            Cmd::ToggleCursorGlide => self.config.cursor_glide ^= true,
            Cmd::ToggleCursorBlink => self.config.cursor_blink ^= true,
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
            Cmd::Launch(id) => self.launch_op(id, crate::launchpad::Op::Run),
            Cmd::NewLauncher => {
                let cwd = self.focused_cwd();
                self.sidebar.new_launcher(cwd);
            }
        }
    }

    /// The Ask popup as its own borderless, always-on-top window, free to move anywhere on the
    /// desktop. Falls back to `ask_popup` inside the main window when that isn't possible.
    fn ask_window(&mut self, ctx: &egui::Context) {
        const PAD: egui::Margin = egui::Margin { left: 18.0, right: 18.0, top: 10.0, bottom: 26.0 };
        if std::mem::take(&mut self.palette.ask_reopen) {
            ctx.request_repaint();
            return;
        }
        let theme = crate::theme::Theme::preset(&self.config.theme);
        let width = 760.0_f32;
        let size = if self.palette.ask_size.x > 0.0 { self.palette.ask_size } else { vec2(width + PAD.left + PAD.right, 220.0) };
        let main = ctx.input(|i| i.viewport().outer_rect);
        if !self.palette.ask_shown {
            self.palette.ask_open_pos = self.palette.ask_last_pos.or_else(|| main.map(|r| pos2(r.center().x - size.x / 2.0, r.min.y + 52.0)));
        }
        self.palette.ask_shown = true;
        let monitor_h = ctx.input(|i| i.viewport().monitor_size.map(|m| m.y)).unwrap_or(ctx.screen_rect().height() + 200.0);
        let max_h = (monitor_h - 420.0).clamp(140.0, 560.0);
        let mut b = egui::ViewportBuilder::default()
            .with_title("Ask S-term")
            .with_app_id("s-term")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_inner_size(size);
        if let Some(p) = self.palette.ask_open_pos {
            b = b.with_position(p);
        }
        let mut close = false;
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("s-term-ask"), b, |ctx, class| {
            if class == egui::ViewportClass::Embedded {
                self.palette.ask_no_window = true;
                return;
            }
            egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
                let r = egui::Frame::none()
                    .inner_margin(PAD)
                    .show(ui, |ui| {
                        crate::glass::frame(ui, &theme, 22.0, theme.elevated, crate::glass::Lift::Float, 12.0, |ui| {
                            ui.set_width(width - 24.0);
                            ui.set_max_width(width - 24.0);
                            self.ask_view(ui, ctx, &theme, max_h);
                        });
                    })
                    .response
                    .rect;
                let new = vec2(r.width().ceil(), r.height().ceil());
                if new != self.palette.ask_size {
                    self.palette.ask_size = new;
                    ctx.request_repaint();
                }
            });
            if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape)) {
                close = true;
            }
            if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
                self.palette.ask_last_pos = Some(r.min);
            }
        });
        if close {
            self.palette.open = false;
        }
    }

    /// The popup in Ask mode, drawn inside the main window.
    fn ask_popup(&mut self, ctx: &egui::Context) {
        let theme = crate::theme::Theme::preset(&self.config.theme);
        let screen = ctx.screen_rect();
        let width = (screen.width() - 40.0).min(760.0);
        // Centered on its real size (wide content can make it grow past `width`), plus wherever
        // it was dragged; kept on screen so the header can always be grabbed again.
        let size = if self.palette.ask_size.x > 0.0 { self.palette.ask_size } else { vec2(width, 0.0) };
        let mut pos = pos2(screen.center().x - size.x / 2.0, screen.min.y + 64.0) + self.palette.ask_offset;
        pos.x = pos.x.clamp(screen.min.x + 80.0 - size.x, (screen.max.x - 80.0).max(screen.min.x));
        pos.y = pos.y.clamp(screen.min.y, (screen.max.y - 48.0).max(screen.min.y));
        let max_h = (screen.height() - 330.0).clamp(140.0, 520.0);
        let mut close = false;
        egui::Area::new(Id::new("s-term-palette-backdrop")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            // Lighter than the command list's backdrop: the panes behind stay readable.
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(45));
            if resp.clicked() && !self.ask.needs_attention() {
                close = true;
            }
        });
        let area = egui::Area::new(Id::new("s-term-palette")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            crate::glass::frame(ui, &theme, 22.0, theme.elevated, crate::glass::Lift::High, 12.0, |ui| {
                ui.set_width(width - 24.0);
                ui.set_max_width(width - 24.0);
                self.ask_view(ui, ctx, &theme, max_h);
            });
        });
        self.palette.ask_size = area.response.rect.size();
        if close {
            self.palette.open = false;
        }
    }

    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        if self.palette.mode == Mode::Ask {
            // Screenshots capture only the main window, so they keep the popup inside it.
            self.palette.ask_windowed = !self.palette.ask_no_window && self.devshot.is_none();
            return if self.palette.ask_windowed { self.ask_window(ctx) } else { self.ask_popup(ctx) };
        }
        self.palette.ask_windowed = false;
        self.palette.ask_shown = false;
        let all = entries(self);
        let mut list: Vec<(i32, &Entry)> = all
            .iter()
            .filter_map(|e| fuzzy(&self.palette.query, &format!("{} {} {}", e.label, e.group, e.search)).map(|s| (s, e)))
            .collect();
        list.sort_by(|a, b| b.0.cmp(&a.0));
        // "Ask S-term" comes first for questions and sentences, second for short queries.
        let q = self.palette.query.trim().to_string();
        let ask_entry = Entry { label: format!("Ask S-term: “{q}”"), group: "Assistant", keys: "", icon: Icon::Sparkle, cmd: Cmd::Ask, search: "" };
        // From the Ask box, commands live in a dropdown under the text field (closed at first).
        let dropdown = self.palette.ask_first;
        let matches = list.len();
        if dropdown && !self.palette.commands_open {
            list.clear();
        }
        if !q.is_empty() {
            let first = dropdown || list.is_empty() || q.ends_with('?') || q.split_whitespace().count() >= 3;
            list.insert(if first { 0 } else { 1.min(list.len()) }, (0, &ask_entry));
        }
        // Where the dropdown header goes: after the Ask row, if there is one.
        let header_at = usize::from(!q.is_empty());
        let n = list.len();
        if n > 0 {
            self.palette.selected = self.palette.selected.min(n - 1);
        }
        let (down, up, enter) = ctx.input_mut(|i| {
            (i.consume_key(Modifiers::NONE, Key::ArrowDown), i.consume_key(Modifiers::NONE, Key::ArrowUp), i.consume_key(Modifiers::NONE, Key::Enter))
        });
        // ↓ past the last row opens the dropdown.
        if down && dropdown && !self.palette.commands_open && self.palette.selected + 1 >= n {
            self.palette.commands_open = true;
            if n > 0 {
                self.palette.selected = n;
            }
        } else if n > 0 {
            if down {
                self.palette.selected = (self.palette.selected + 1) % n;
            }
            if up {
                self.palette.selected = (self.palette.selected + n - 1) % n;
            }
        }
        let mut run: Option<Cmd> = (enter && self.palette.selected < n).then(|| list[self.palette.selected].1.cmd.clone());
        let mut toggle_commands = false;
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
                            .hint_text(if self.palette.ask_first { "Ask S-term anything, or search commands/settings…" } else { "Search commands/settings, or ask S-term…" })
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
                        if list.is_empty() && !dropdown {
                            ui.add_space(8.0);
                            ui.label(RichText::new("No matching commands/settings").color(theme.muted));
                        }
                        for (i, (_, e)) in list.iter().enumerate() {
                            if dropdown && i == header_at {
                                toggle_commands |= commands_header(ui, theme, self.palette.commands_open, matches, q.is_empty());
                            }
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
                            let ink = lerp_color(theme.muted, theme.accent, t);
                            if matches!(e.cmd, Cmd::Ask) {
                                super::chrome::paint_glyph(p, self.logo.id(), icons::center_rect(ic.center(), 18.0), ink);
                            } else {
                                icons::draw(p, e.icon, icons::center_rect(ic.center(), 14.0), ink);
                            }
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
                        if dropdown && list.len() <= header_at {
                            toggle_commands |= commands_header(ui, theme, self.palette.commands_open, matches, q.is_empty());
                        }
                        if dropdown && self.palette.commands_open && matches == 0 {
                            ui.label(RichText::new("   No matching commands/settings").size(12.0).color(theme.muted));
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let hint = if dropdown && q.is_empty() { "type a question   ⏎ ask   esc close" } else { "↑↓ navigate   ⏎ run   esc close" };
                        ui.label(RichText::new(hint).size(10.5).color(theme.muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(format!("{} commands/settings", all.len())).size(10.5).color(theme.muted));
                        });
                    });
                });
        });

        if toggle_commands {
            self.palette.commands_open ^= true;
            self.palette.just_opened = true; // keep typing in the field
        }
        if let Some(cmd) = run {
            if matches!(cmd, Cmd::Ask) {
                let q = std::mem::take(&mut self.palette.query);
                self.palette.open_ask();
                if !self.ask.busy() {
                    self.ask_send(q);
                } else {
                    self.ask.input = q;
                }
                return;
            }
            self.palette.open = false;
            self.run_cmd(cmd, ctx);
        } else if close {
            self.palette.open = false;
        }
    }
}

/// The "Commands" dropdown header under the text field. Returns true when clicked.
fn commands_header(ui: &mut egui::Ui, theme: &crate::theme::Theme, open: bool, matches: usize, all: bool) -> bool {
    ui.add_space(2.0);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(Id::new("s-term-pal-cmds"), resp.hovered(), 0.1);
    let p = ui.painter();
    if hover > 0.0 {
        p.rect_filled(r, 7.0, theme.header.gamma_multiply(hover));
    }
    let icon = if open { Icon::ChevronDown } else { Icon::ChevronRight };
    icons::draw(p, icon, icons::center_rect(pos2(r.min.x + 14.0, r.center().y), 11.0), theme.muted);
    let label = match (all, matches) {
        (true, _) => "Commands/Settings".to_string(),
        (false, 1) => "1 match".to_string(),
        (false, n) => format!("{n} matches"),
    };
    let col = lerp_color(theme.muted, theme.text, hover);
    p.text(pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(12.0), col);
    if !all || !open {
        p.text(pos2(r.max.x - 10.0, r.center().y), Align2::RIGHT_CENTER, if open { "hide" } else { "show" }, FontId::proportional(11.0), theme.muted.gamma_multiply(0.8));
    }
    ui.add_space(2.0);
    resp.clicked()
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

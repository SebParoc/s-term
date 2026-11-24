//! The S-term window: tabs, the pane tree, input dispatch and the overall frame layout.
//!
//! Submodules split the UI by area:
//! - `chrome`   : title bar (tabs, window controls), activity rail, status bar, frame resizing
//! - `panes`    : the terminal panes (layout animation, mouse, painting)
//! - `palette`  : the command palette (Ctrl+Shift+P)
//! - `settings` : the settings page (Ctrl+,)

mod chrome;
mod devshot;
mod palette;
mod panes;
mod settings;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use egui::{vec2, Color32, FontId, Key, Rect, Vec2, ViewportCommand};

use crate::config::Config;
use crate::fonts;
use crate::input::{self, Action, Dir};
use crate::layout::{Node, Orientation};
use crate::pane::{GridSize, Pane, PaneId};
use crate::render::GlyphCache;
use crate::sidebar::{Sidebar, UiAction};
use crate::theme::Theme;

/// Clickable but never keyboard-focusable, so Tab/Space/Enter always go to the shell.
pub(crate) const CLICK: egui::Sense = egui::Sense { click: true, drag: false, focusable: false };
pub(crate) const CLICK_DRAG: egui::Sense = egui::Sense { click: true, drag: true, focusable: false };
pub(crate) const DRAG: egui::Sense = egui::Sense { click: false, drag: true, focusable: false };

struct Tab {
    root: Node,
    focus: PaneId,
    zoomed: Option<PaneId>,
    broadcast: bool,
}

pub struct STerm {
    ctx: egui::Context,
    theme: Theme,
    /// Settings as edited (the UI writes here)…
    config: Config,
    /// …and as last applied/saved.
    applied: Config,
    tabs: Vec<Tab>,
    active: usize,
    panes: HashMap<PaneId, Pane>,
    next_id: PaneId,
    tx: Sender<(PaneId, TermEvent)>,
    rx: Receiver<(PaneId, TermEvent)>,
    metrics_for: (f32, f32, f32),
    cell: Vec2,
    glyphs: GlyphCache,
    faux_bold: bool,
    /// On-screen (animated) pane rects, gliding toward their layout targets.
    anim_rects: HashMap<PaneId, Rect>,
    /// Freshly split panes and the direction they grow in from.
    spawn_from: HashMap<PaneId, Orientation>,
    last_area: Rect,
    last_active: usize,
    tab_fade: f32,
    dragging_divider: bool,
    sidebar: Sidebar,
    /// Terminal area of the last frame (overlays float over it).
    central: Rect,
    /// Pane rectangles from the last frame (for directional focus).
    rects: Vec<(PaneId, Rect)>,
    window_title: String,
    palette: palette::Palette,
    settings_open: bool,
    settings: settings::SettingsUi,
    last_follow: Instant,
    /// A window-edge resize started this frame (don't also start a title-bar drag).
    frame_resizing: bool,
    devshot: Option<devshot::DevShot>,
    /// The scorpion logo (white glyph + alpha, tinted when drawn).
    logo: egui::TextureHandle,
    /// Accumulated Ctrl+wheel zoom not yet applied.
    zoom_accum: f32,
    /// "Font 15px" / "Zoom 110%" indicator and when it was triggered.
    zoom_hud: Option<(String, Instant)>,
}

impl STerm {
    pub fn new(cc: &eframe::CreationContext<'_>, config: Config) -> Self {
        let ctx = cc.egui_ctx.clone();
        let theme = Theme::preset(&config.theme);
        theme.apply_visuals(&ctx);
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.style_mut(|s| {
            s.animation_time = if config.animations { 1.0 / 10.0 } else { 0.0 };
            s.spacing.button_padding = vec2(10.0, 5.0);
        });
        let font = fonts::install(&ctx, &config.font_family);

        let (tx, rx) = channel();
        let mut app = Self {
            ctx: ctx.clone(),
            theme,
            applied: config.clone(),
            glyphs: GlyphCache::new(config.font_size),
            config,
            tabs: Vec::new(),
            active: 0,
            panes: HashMap::new(),
            next_id: 1,
            tx,
            rx,
            metrics_for: (0.0, 0.0, 0.0),
            cell: vec2(8.0, 17.0),
            faux_bold: font.faux_bold,
            anim_rects: HashMap::new(),
            spawn_from: HashMap::new(),
            last_area: Rect::NOTHING,
            last_active: 0,
            tab_fade: 0.0,
            dragging_divider: false,
            sidebar: Sidebar::new(ctx.clone()),
            central: Rect::NOTHING,
            rects: Vec::new(),
            window_title: String::new(),
            palette: Default::default(),
            settings_open: false,
            settings: Default::default(),
            last_follow: Instant::now() - Duration::from_secs(10),
            frame_resizing: false,
            devshot: devshot::DevShot::from_env(),
            logo: ctx.load_texture(
                "s-term-logo",
                egui::ColorImage::from_rgba_unmultiplied([256, 256], include_bytes!("../assets/icon/glyph-256.rgba")),
                egui::TextureOptions::LINEAR,
            ),
            zoom_accum: 1.0,
            zoom_hud: None,
        };
        if app.config.zoom_ui {
            ctx.set_zoom_factor(app.config.ui_scale);
        }
        if app.devshot.is_none() {
            app.config.write_prompt_flags();
        }
        app.sidebar.open = app.config.panel_on_start;
        app.new_tab();
        app
    }

    // ---------------------------------------------------------------- settings

    /// Applies whatever changed in `config` since the last frame, and saves it.
    fn apply_config(&mut self, ctx: &egui::Context) {
        if self.config == self.applied {
            return;
        }
        let (new, old) = (&self.config, &self.applied);
        if new.theme != old.theme {
            self.theme = Theme::preset(&new.theme);
            self.theme.apply_visuals(ctx);
        }
        if new.font_family != old.font_family {
            self.faux_bold = fonts::install(ctx, &new.font_family).faux_bold;
            self.metrics_for = (0.0, 0.0, 0.0);
        }
        if new.animations != old.animations {
            let on = new.animations;
            ctx.style_mut(|s| s.animation_time = if on { 1.0 / 10.0 } else { 0.0 });
        }
        if new.zoom_ui != old.zoom_ui || new.ui_scale != old.ui_scale {
            ctx.set_zoom_factor(if new.zoom_ui { new.ui_scale } else { 1.0 });
        }
        if new.system_titlebar != old.system_titlebar {
            ctx.send_viewport_cmd(ViewportCommand::Decorations(new.system_titlebar));
        }
        // Screenshot runs (S_TERM_SCREENSHOT) must not touch the user's settings.
        if self.devshot.is_none() {
            self.config.save();
            if new.prompt != old.prompt {
                self.config.write_prompt_flags();
            }
        }
        self.applied = self.config.clone();
    }

    // ---------------------------------------------------------------- panes & tabs

    fn spawn_pane(&mut self, cwd: Option<PathBuf>) -> Option<PaneId> {
        let id = self.next_id;
        self.next_id += 1;
        let size = GridSize { cols: 80, rows: 24 };
        let (shell, scrollback) = (self.config.shell.clone(), self.config.scrollback);
        match Pane::spawn(id, self.tx.clone(), self.ctx.clone(), cwd, size, self.cell, &shell, scrollback) {
            Ok(p) => {
                self.panes.insert(id, p);
                Some(id)
            }
            Err(e) => {
                eprintln!("s-term: failed to spawn shell: {e:#}");
                None
            }
        }
    }

    fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    fn focused(&self) -> Option<&Pane> {
        self.tab().and_then(|t| self.panes.get(&t.focus))
    }

    fn focused_cwd(&self) -> Option<PathBuf> {
        self.focused().and_then(|p| p.cwd())
    }

    fn new_tab(&mut self) {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let cwd = if self.config.new_tab_home && !self.tabs.is_empty() { home } else { self.focused_cwd() };
        if let Some(id) = self.spawn_pane(cwd) {
            self.tabs.push(Tab { root: Node::Leaf(id), focus: id, zoomed: None, broadcast: false });
            self.active = self.tabs.len() - 1;
        }
    }

    /// Starts another S-term process, opened in the focused pane's folder.
    fn new_window(&self) {
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => return eprintln!("s-term: new window: {e}"),
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.env_remove("S_TERM_SCREENSHOT").env_remove("S_TERM_DEMO");
        if let Some(cwd) = self.focused_cwd().filter(|d| d.is_dir()) {
            cmd.current_dir(cwd);
        }
        match cmd.spawn() {
            // Reap it when it closes so it doesn't linger as a zombie.
            Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
            Err(e) => eprintln!("s-term: new window: {e}"),
        }
    }

    fn split(&mut self, orient: Orientation) {
        let cwd = self.focused_cwd();
        if self.tabs.is_empty() {
            return;
        }
        if let Some(new) = self.spawn_pane(cwd) {
            let tab = &mut self.tabs[self.active];
            tab.root.split(tab.focus, new, orient);
            tab.focus = new;
            self.spawn_from.insert(new, orient);
            tab.zoomed = None;
        }
    }

    fn close_pane(&mut self, id: PaneId) {
        if self.panes.remove(&id).is_none() {
            return;
        }
        let Some(ti) = self.tabs.iter().position(|t| t.root.contains(id)) else { return };
        let tab = &mut self.tabs[ti];
        let root = std::mem::replace(&mut tab.root, Node::Leaf(PaneId::MAX));
        match root.remove(id) {
            Some(r) => {
                tab.root = r;
                if tab.focus == id {
                    tab.focus = tab.root.first_leaf();
                }
                if tab.zoomed == Some(id) {
                    tab.zoomed = None;
                }
            }
            None => {
                self.tabs.remove(ti);
                if ti < self.active {
                    self.active -= 1;
                }
                self.active = self.active.min(self.tabs.len().saturating_sub(1));
            }
        }
    }

    fn close_tab(&mut self, ti: usize) {
        let mut ids = Vec::new();
        if let Some(t) = self.tabs.get(ti) {
            t.root.leaves(&mut ids);
        }
        for id in ids {
            self.close_pane(id);
        }
    }

    fn cycle_pane(&mut self, forward: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let mut ids = Vec::new();
        tab.root.leaves(&mut ids);
        if let Some(i) = ids.iter().position(|&x| x == tab.focus) {
            let n = ids.len();
            tab.focus = ids[if forward { (i + 1) % n } else { (i + n - 1) % n }];
            if tab.zoomed.is_some() {
                tab.zoomed = Some(tab.focus);
            }
        }
    }

    /// Nearest pane in a direction, among those overlapping on the perpendicular axis.
    fn neighbor(&self, dir: Dir) -> Option<PaneId> {
        let focus = self.tab()?.focus;
        let cur = self.rects.iter().find(|(id, _)| *id == focus)?.1;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| a1.min(b1) - a0.max(b0);
        self.rects
            .iter()
            .filter(|(id, _)| *id != focus)
            .filter_map(|(id, r)| {
                let (dist, ov) = match dir {
                    Dir::Left => (cur.min.x - r.max.x, overlap(cur.min.y, cur.max.y, r.min.y, r.max.y)),
                    Dir::Right => (r.min.x - cur.max.x, overlap(cur.min.y, cur.max.y, r.min.y, r.max.y)),
                    Dir::Up => (cur.min.y - r.max.y, overlap(cur.min.x, cur.max.x, r.min.x, r.max.x)),
                    Dir::Down => (r.min.y - cur.max.y, overlap(cur.min.x, cur.max.x, r.min.x, r.max.x)),
                };
                (dist >= -1.0 && ov > 0.0).then_some((*id, dist, ov))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|(id, _, _)| id)
    }

    // ---------------------------------------------------------------- input

    fn send_input(&mut self, bytes: Vec<u8>) {
        let Some(tab) = self.tab() else { return };
        let mut targets = Vec::new();
        if tab.broadcast {
            tab.root.leaves(&mut targets);
        } else {
            targets.push(tab.focus);
        }
        for id in targets {
            if let Some(p) = self.panes.get(&id) {
                p.term.lock().scroll_display(Scroll::Bottom);
                p.write(bytes.clone());
            }
        }
    }

    fn focused_mode(&self) -> TermMode {
        self.focused().map(|p| *p.term.lock().mode()).unwrap_or(TermMode::empty())
    }

    fn paste(&mut self, text: &str) {
        let bytes = if self.focused_mode().contains(TermMode::BRACKETED_PASTE) {
            let clean: String = text.chars().filter(|&c| c != '\x1b').collect();
            format!("\x1b[200~{clean}\x1b[201~").into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.send_input(bytes);
    }

    fn copy_selection(&self, ctx: &egui::Context) {
        if let Some(s) = self.focused().and_then(|p| p.term.lock().selection_to_string()) {
            ctx.output_mut(|o| o.copied_text = s);
        }
    }

    fn scroll_focused(&self, scroll: Scroll) {
        if let Some(p) = self.focused() {
            p.term.lock().scroll_display(scroll);
        }
    }

    fn clear_scrollback(&self) {
        if let Some(p) = self.focused() {
            p.term.lock().grid_mut().clear_history();
        }
    }

    fn do_action(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::SplitStacked => self.split(Orientation::Stacked),
            Action::SplitSideBySide => self.split(Orientation::SideBySide),
            Action::ClosePane => {
                if let Some(id) = self.tab().map(|t| t.focus) {
                    self.close_pane(id);
                }
            }
            Action::NewTab => self.new_tab(),
            Action::NewWindow => self.new_window(),
            Action::NextTab | Action::PrevTab if !self.tabs.is_empty() => {
                let n = self.tabs.len();
                self.active = if action == Action::NextTab { (self.active + 1) % n } else { (self.active + n - 1) % n };
            }
            Action::NextPane => self.cycle_pane(true),
            Action::PrevPane => self.cycle_pane(false),
            Action::Focus(d) => {
                if let Some(id) = self.neighbor(d) {
                    self.tabs[self.active].focus = id;
                }
            }
            Action::Resize(d) => {
                let (o, delta) = match d {
                    Dir::Left => (Orientation::SideBySide, -0.05),
                    Dir::Right => (Orientation::SideBySide, 0.05),
                    Dir::Up => (Orientation::Stacked, -0.05),
                    Dir::Down => (Orientation::Stacked, 0.05),
                };
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.root.adjust(tab.focus, o, delta);
                }
            }
            Action::ToggleZoom => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.zoomed = if tab.zoomed.is_some() { None } else { Some(tab.focus) };
                }
            }
            Action::FontBigger => self.zoom_step(1),
            Action::FontSmaller => self.zoom_step(-1),
            Action::FontReset => self.zoom_step(0),
            Action::ScrollPageUp => self.scroll_focused(Scroll::PageUp),
            Action::ScrollPageDown => self.scroll_focused(Scroll::PageDown),
            Action::ScrollTop => self.scroll_focused(Scroll::Top),
            Action::ScrollBottom => self.scroll_focused(Scroll::Bottom),
            Action::BroadcastAll | Action::BroadcastOff => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.broadcast = action == Action::BroadcastAll;
                }
            }
            Action::ToggleSidebar => self.sidebar.toggle(),
            Action::OpenPalette => self.palette.toggle(),
            Action::OpenSettings => self.settings_open = !self.settings_open,
            Action::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            _ => {}
        }
    }

    /// One zoom step: +1 bigger, -1 smaller, 0 reset. Scales the text or the whole UI (setting).
    fn zoom_step(&mut self, dir: i32) {
        let c = &mut self.config;
        let label = if c.zoom_ui {
            c.ui_scale = match dir {
                0 => 1.0,
                d => ((c.ui_scale + 0.1 * d as f32) * 20.0).round() / 20.0,
            }
            .clamp(0.6, 2.0);
            format!("Zoom {:.0}%", c.ui_scale * 100.0)
        } else {
            c.font_size = match dir {
                0 => Config::default().font_size,
                d => c.font_size + d as f32,
            }
            .clamp(6.0, 48.0);
            format!("Font {}px", c.font_size)
        };
        self.zoom_hud = Some((label, Instant::now()));
    }

    /// Ctrl + mouse wheel (or pinch) zooms, one step per notch.
    fn wheel_zoom(&mut self, ctx: &egui::Context) {
        if self.modal_open() {
            return;
        }
        let z = ctx.input(|i| i.zoom_delta());
        if z == 1.0 {
            return;
        }
        self.zoom_accum *= z;
        while self.zoom_accum >= 1.12 {
            self.zoom_accum /= 1.12;
            self.zoom_step(1);
        }
        while self.zoom_accum <= 1.0 / 1.12 {
            self.zoom_accum *= 1.12;
            self.zoom_step(-1);
        }
    }

    /// Small pill showing the zoom level, fading out.
    fn zoom_hud(&mut self, ctx: &egui::Context) {
        let Some((label, at)) = &self.zoom_hud else { return };
        let age = at.elapsed().as_secs_f32();
        if age > 1.3 {
            self.zoom_hud = None;
            return;
        }
        let alpha = if age < 0.9 { 1.0 } else { 1.0 - (age - 0.9) / 0.4 };
        ctx.request_repaint();
        let theme = &self.theme;
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("s-term-zoom-hud")));
        let g = p.layout_no_wrap(label.clone(), FontId::proportional(15.0), theme.text.gamma_multiply(alpha));
        let c = self.central.center();
        let r = Rect::from_center_size(c, g.size() + vec2(56.0, 22.0));
        p.rect_filled(r, 12.0, theme.elevated.gamma_multiply(0.96 * alpha));
        p.rect_stroke(r.shrink(0.5), 12.0, egui::Stroke::new(1.0_f32, theme.border.gamma_multiply(alpha)));
        crate::icons::draw(&p, crate::icons::Icon::Search, crate::icons::center_rect(egui::pos2(r.min.x + 22.0, c.y), 16.0), theme.accent.gamma_multiply(alpha));
        p.galley(egui::pos2(r.min.x + 40.0, c.y - g.size().y / 2.0), g, theme.text);
    }

    fn modal_open(&self) -> bool {
        self.palette.open || self.settings_open || self.sidebar.viewer_active()
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        let (events, mods) = ctx.input(|i| (i.events.clone(), i.modifiers));
        if self.modal_open() {
            // Overlays own the keyboard: Esc closes the top one, nothing reaches the shell.
            for ev in events {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = ev {
                    match input::action_for(key, modifiers) {
                        Some(Action::OpenPalette) => self.palette.toggle(),
                        Some(Action::OpenSettings) => {
                            self.palette.open = false;
                            self.settings_open = !self.settings_open;
                        }
                        _ if key == Key::Escape => {
                            if self.palette.open {
                                self.palette.open = false;
                            } else if self.sidebar.viewer_active() {
                                self.sidebar.close_viewer();
                            } else {
                                self.settings_open = false;
                            }
                        }
                        _ => {}
                    }
                }
            }
            return;
        }
        let mut suppress_text = false;
        for ev in events {
            match ev {
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    if let Some(a) = input::action_for(key, modifiers) {
                        self.do_action(a, ctx);
                        continue;
                    }
                    // Ctrl(+Shift)+C/X/V come through as Copy/Cut/Paste events below.
                    if modifiers.ctrl && matches!(key, Key::C | Key::X | Key::V) {
                        continue;
                    }
                    let app_cursor = self.focused_mode().contains(TermMode::APP_CURSOR);
                    if let Some((bytes, sup)) = input::key_to_bytes(key, modifiers, app_cursor) {
                        suppress_text |= sup;
                        self.send_input(bytes);
                    }
                }
                egui::Event::Text(t) => {
                    if std::mem::take(&mut suppress_text) {
                        continue;
                    }
                    self.send_input(t.into_bytes());
                }
                egui::Event::Copy => {
                    if mods.shift {
                        self.copy_selection(ctx);
                    } else {
                        self.send_input(vec![0x03]); // Ctrl+C -> SIGINT
                    }
                }
                egui::Event::Cut => {
                    if !mods.shift {
                        self.send_input(vec![0x18]);
                    }
                }
                egui::Event::Paste(s) => {
                    if mods.shift {
                        self.paste(&s);
                    } else {
                        self.send_input(vec![0x16]); // Ctrl+V (e.g. image paste in Claude Code)
                    }
                }
                _ => {}
            }
        }
    }

    // ---------------------------------------------------------------- terminal events

    fn process_term_events(&mut self, ctx: &egui::Context) {
        let mut dead = Vec::new();
        while let Ok((id, ev)) = self.rx.try_recv() {
            let Some(pane) = self.panes.get_mut(&id) else { continue };
            match ev {
                TermEvent::Title(t) => pane.title = t,
                TermEvent::ResetTitle => pane.title = "shell".into(),
                TermEvent::PtyWrite(s) => pane.write(s.into_bytes()),
                TermEvent::ClipboardStore(_, s) => ctx.output_mut(|o| o.copied_text = s),
                TermEvent::ColorRequest(i, fmt) => {
                    let rgb = self.theme.index(i, pane.term.lock().colors());
                    pane.write(fmt(rgb).into_bytes());
                }
                TermEvent::TextAreaSizeRequest(fmt) => {
                    let s = fmt(pane.window_size());
                    pane.write(s.into_bytes());
                }
                TermEvent::ChildExit(_) | TermEvent::Exit => dead.push(id),
                _ => {}
            }
        }
        for id in dead {
            self.close_pane(id);
        }
    }

    fn run_sidebar_actions(&mut self, ctx: &egui::Context, actions: Vec<UiAction>) {
        for a in actions {
            match a {
                UiAction::Insert(text) => self.send_input(text.into_bytes()),
                UiAction::Copy(text) => ctx.output_mut(|o| o.copied_text = text),
                UiAction::OpenExternal(path) => {
                    if let Err(e) = std::process::Command::new("xdg-open").arg(&path).spawn() {
                        eprintln!("s-term: xdg-open {}: {e}", path.display());
                    }
                }
                UiAction::Drop(path, pos) => self.drop_paths(&[path], pos),
            }
        }
    }

    /// Pane under a screen position.
    fn pane_at(&self, pos: egui::Pos2) -> Option<crate::pane::PaneId> {
        self.rects.iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| *id)
    }

    /// Files dropped on a pane: focus it and paste their absolute paths (Claude Code turns
    /// pasted image paths into attachments).
    fn drop_paths(&mut self, paths: &[std::path::PathBuf], pos: egui::Pos2) {
        let Some(id) = self.pane_at(pos) else { return };
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.focus = id;
        }
        let text: Vec<String> = paths.iter().map(|p| crate::sidebar::quote(&p.to_string_lossy())).collect();
        self.paste(&format!("{} ", text.join(" ")));
    }

    /// Files dragged in from the desktop / file manager.
    fn os_file_drops(&mut self, ctx: &egui::Context) {
        let (files, pos) = ctx.input(|i| (i.raw.dropped_files.clone(), i.pointer.latest_pos()));
        let paths: Vec<_> = files.into_iter().filter_map(|f| f.path).collect();
        if paths.is_empty() {
            return;
        }
        let pos = pos.or_else(|| self.rects.iter().find(|(id, _)| Some(*id) == self.tab().map(|t| t.focus)).map(|(_, r)| r.center()));
        if let Some(pos) = pos {
            self.drop_paths(&paths, pos);
        }
    }

    // ---------------------------------------------------------------- metrics & title

    fn ensure_metrics(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let key = (self.config.font_size, ppp, self.config.line_height);
        if self.metrics_for == key {
            return;
        }
        let font = FontId::monospace(self.config.font_size);
        let (w, h) = ctx.fonts(|f| {
            (f.layout_no_wrap("M".into(), font.clone(), Color32::WHITE).size().x, f.row_height(&font))
        });
        // Whole physical pixels per cell, so every glyph lands on the pixel grid: crisp + evenly spaced.
        let snap = |v: f32| (v * ppp).round().max(1.0) / ppp;
        self.cell = vec2(snap(w), snap(h * self.config.line_height));
        self.glyphs.reset(self.config.font_size);
        self.metrics_for = key;
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.focused() {
            Some(p) => format!("{} — S-term", p.title),
            None => "S-term".into(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }
}

impl eframe::App for STerm {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_config(ctx);
        self.ensure_metrics(ctx);
        self.process_term_events(ctx);
        self.handle_keyboard(ctx);
        self.wheel_zoom(ctx);
        if self.tabs.is_empty() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        if self.last_follow.elapsed() > Duration::from_millis(500) {
            self.last_follow = Instant::now();
            let cwd = self.focused_cwd();
            self.sidebar.follow(cwd);
        }

        self.frame_resize(ctx);
        let scene = ctx.layer_painter(egui::LayerId::background());
        crate::glass::backdrop(ctx, &scene, ctx.screen_rect(), self.window_rounding(ctx), &self.theme);
        self.title_bar(ctx);
        if self.tabs.is_empty() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        if self.config.status_bar {
            self.status_bar(ctx);
        }
        self.rail(ctx);
        let actions = self.sidebar.show(ctx, &self.theme, &mut self.config);
        self.run_sidebar_actions(ctx, actions);
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| self.panes_ui(ui));

        let actions = self.sidebar.show_viewer(ctx, &self.theme, self.central);
        self.run_sidebar_actions(ctx, actions);
        self.os_file_drops(ctx);
        if self.settings_open {
            self.settings_page(ctx);
        }
        if self.palette.open {
            self.palette_ui(ctx);
        }
        self.zoom_hud(ctx);
        self.window_border(ctx);
        self.update_title(ctx);
        self.devshot(ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        if !self.config.system_titlebar {
            // Transparent, so the rounded corners show the desktop behind them.
            return [0.0; 4];
        }
        let c = self.theme.bar;
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0]
    }
}

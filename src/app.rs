//! The S-term window: tabs, the pane tree, input dispatch and the overall frame layout.
//!
//! Submodules split the UI by area:
//! - `chrome`   : title bar (tabs, window controls), activity rail, status bar, frame resizing
//! - `panes`    : the terminal panes (layout animation, mouse, painting)
//! - `palette`  : the command palette (Ctrl+Shift+P)
//! - `settings` : the settings page (Ctrl+,)
//! - `launch`   : runs the Launchpad's launchers in their panes
//! - `history`  : records closed tabs and the session, reopens them

mod ask;
mod chrome;
mod devshot;
mod history;
mod launch;
mod palette;
mod panes;
mod settings;
mod setup;

pub(crate) use settings::segmented;

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
use crate::theme::{TermColors, Theme};

/// Clickable but never keyboard-focusable, so Tab/Space/Enter always go to the shell.
pub(crate) use s_style::chrome::{CLICK, CLICK_DRAG, DRAG};

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
    /// System fonts found for characters the installed ones lack.
    fallbacks: fonts::Fallbacks,
    /// `set_fonts` was called last frame, so the new fonts are live from this one.
    fonts_changed: bool,
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
    /// The tab bar's "+" button from the last frame (a folder dropped on it opens a new tab there).
    plus_rect: Rect,
    window_title: String,
    palette: palette::Palette,
    ask: ask::Ask,
    /// First-run setup screen.
    setup: Option<setup::Setup>,
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
    /// Launchpad launchers that have a pane, by launcher id.
    launches: HashMap<u64, launch::Run>,
    /// What those panes are doing, refreshed a few times a second.
    launch_status: crate::launchpad::Statuses,
    launch_checked: Option<Instant>,
    /// Closed tabs and the last session (the History view).
    hist: history::HistState,
    /// The other S-term windows: their agents, and requests to show one of this window's panes.
    peers: crate::peers::Peers,
    /// A V press or a Paste event arrived since the last V release (see `handle_keyboard`).
    v_seen: bool,
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
        let fallbacks = fonts::Fallbacks::new(&ctx);
        let font = fonts::install(&ctx, &config.font_family, &fallbacks);

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
            fallbacks,
            fonts_changed: false,
            anim_rects: HashMap::new(),
            spawn_from: HashMap::new(),
            last_area: Rect::NOTHING,
            last_active: 0,
            tab_fade: 0.0,
            dragging_divider: false,
            sidebar: Sidebar::new(ctx.clone()),
            central: Rect::NOTHING,
            rects: Vec::new(),
            plus_rect: Rect::NOTHING,
            window_title: String::new(),
            palette: Default::default(),
            ask: Default::default(),
            setup: None,
            settings_open: false,
            settings: Default::default(),
            last_follow: Instant::now() - Duration::from_secs(10),
            frame_resizing: false,
            // Screenshots stay out of the other windows' lists.
            peers: crate::peers::Peers::new(ctx.clone(), std::env::var_os("S_TERM_SCREENSHOT").is_none()),
            devshot: devshot::DevShot::from_env(),
            logo: ctx.load_texture(
                "s-term-logo",
                egui::ColorImage::from_rgba_unmultiplied([256, 256], s_style::brand::GLYPH_256),
                egui::TextureOptions::LINEAR,
            ),
            zoom_accum: 1.0,
            zoom_hud: None,
            launches: HashMap::new(),
            launch_status: Default::default(),
            launch_checked: None,
            hist: Default::default(),
            v_seen: false,
        };
        if app.config.zoom_ui {
            ctx.set_zoom_factor(app.config.ui_scale);
        }
        if app.devshot.is_none() {
            app.config.write_prompt_flags();
        }
        app.sidebar.open = app.config.panel_on_start;
        if !app.config.setup_done && app.devshot.is_none() {
            app.setup = Some(Default::default());
        }
        app.new_tab();
        app.history_start();
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
            self.faux_bold = fonts::install(ctx, &new.font_family, &self.fallbacks).faux_bold;
            self.fonts_changed = true;
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

    /// Finds the AI CLIs running in this window's panes, in tab order, and shares them with the
    /// other S-term windows. The AI view lists them followed by the other windows' ones, and the
    /// usage worker looks up each one's conversation from its process group.
    fn track_agents(&mut self) {
        let mut list = Vec::new();
        for (ti, tab) in self.tabs.iter().enumerate() {
            let mut leaves = Vec::new();
            tab.root.leaves(&mut leaves);
            for id in leaves {
                let Some(p) = self.panes.get(&id) else { continue };
                let (Some(tool), Some(pgid)) = (p.ai_tool(), p.foreground_pgid()) else { continue };
                list.push(crate::usage_ui::AgentPane { window: None, pane: id, tool, pgid, tab: ti, title: p.title.clone() });
            }
        }
        self.peers.set_mine(&list);
        list.extend(self.peers.others().iter().cloned());
        self.sidebar.usage.set_agents(list.iter().map(|a| crate::usage::AgentProc { tool: a.tool, pgid: a.pgid }).collect());
        self.sidebar.agents = list;
    }

    /// Another window's AI view asked to see one of this window's panes: show it and raise the
    /// window (the compositor may only flag it as wanting attention instead).
    fn peer_requests(&mut self, ctx: &egui::Context) {
        while let Some(pane) = self.peers.focus_request() {
            if self.focus_pane(pane) {
                ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
        }
    }

    fn new_tab(&mut self) {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let cwd = if self.config.new_tab_home && !self.tabs.is_empty() { home } else { self.focused_cwd() };
        self.open_tab_at(cwd);
    }

    fn open_tab_at(&mut self, cwd: Option<PathBuf>) -> Option<PaneId> {
        let id = self.spawn_pane(cwd)?;
        self.tabs.push(Tab { root: Node::Leaf(id), focus: id, zoomed: None, broadcast: false });
        self.active = self.tabs.len() - 1;
        Some(id)
    }

    /// Starts another S-term process, opened in the focused pane's folder.
    fn new_window(&self) {
        let exe = resolve_self_exe(std::env::current_exe().ok(), std::env::args_os().next());
        let cwd = self.focused_cwd().filter(|d| d.is_dir());
        let spawn = |path: &std::path::Path| {
            let mut cmd = std::process::Command::new(path);
            cmd.env_remove("S_TERM_SCREENSHOT").env_remove("S_TERM_DEMO");
            if let Some(dir) = &cwd {
                cmd.current_dir(dir);
            }
            cmd.spawn()
        };
        let proc_exe = std::path::Path::new("/proc/self/exe");
        let res = spawn(&exe).or_else(|e| if exe == proc_exe { Err(e) } else { spawn(proc_exe) });
        match res {
            // Reap it when it closes so it doesn't linger as a zombie.
            Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
            Err(e) => eprintln!("s-term: new window: {e}"),
        }
    }

    fn split(&mut self, orient: Orientation) {
        let cwd = self.focused_cwd();
        self.split_at(orient, cwd);
    }

    fn split_at(&mut self, orient: Orientation, cwd: Option<PathBuf>) -> Option<PaneId> {
        if self.tabs.is_empty() {
            return None;
        }
        let new = self.spawn_pane(cwd)?;
        let tab = &mut self.tabs[self.active];
        tab.root.split(tab.focus, new, orient);
        tab.focus = new;
        self.spawn_from.insert(new, orient);
        tab.zoomed = None;
        Some(new)
    }

    /// Closes a pane (its tab too when it's the last one there), keeping it in the History.
    fn close_pane(&mut self, id: PaneId) {
        self.remember_pane(id);
        self.drop_pane(id);
    }

    /// Closes a pane without keeping it in the History.
    fn drop_pane(&mut self, id: PaneId) {
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
        self.remember_tab(ti);
        for id in ids {
            self.drop_pane(id);
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
        self.hist.typed();
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
            Action::ReopenClosed => self.reopen_last(),
            Action::ShowHistory => self.sidebar.toggle_view(crate::sidebar::Tab::History),
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
        // The Ask popup in its own window leaves the terminal usable.
        let ask_floating = self.palette.mode == palette::Mode::Ask && self.palette.ask_windowed;
        (self.palette.open && !ask_floating) || self.settings_open || self.setup.is_some() || self.sidebar.viewer_active()
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        let (events, mods) = ctx.input(|i| (i.events.clone(), i.modifiers));
        // egui-winit swallows a Ctrl+V press whole and sends a Paste event only when the clipboard
        // has text, so with just an image on it (a screenshot) nothing arrives and Claude Code never
        // gets its Ctrl+V. The V release still comes through: one with no press or Paste before it
        // is that Ctrl+V. (Tracked here, before the modal check, so the pairs stay in step.)
        let mut image_ctrl_v = false;
        for ev in &events {
            match ev {
                egui::Event::Key { key: Key::V, pressed: true, .. } | egui::Event::Paste(_) => self.v_seen = true,
                egui::Event::Key { key: Key::V, pressed: false, modifiers, .. } => image_ctrl_v |= !std::mem::take(&mut self.v_seen) && !modifiers.shift,
                _ => {}
            }
        }
        // So does a text field in the side panel (it reports this from the last frame).
        if self.modal_open() || self.sidebar.typing() {
            // Overlays own the keyboard: Esc closes the top one, nothing reaches the shell.
            for ev in events {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = ev {
                    if self.setup.is_some() {
                        continue;
                    }
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
        if image_ctrl_v {
            self.send_input(vec![0x16]);
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
                TermEvent::Title(t) => {
                    self.fallbacks.want_text(ctx, &t);
                    pane.title = t;
                }
                TermEvent::ResetTitle => pane.title = "shell".into(),
                TermEvent::PtyWrite(s) => pane.write(s.into_bytes()),
                TermEvent::ClipboardStore(_, s) => ctx.output_mut(|o| o.copied_text = s),
                TermEvent::ColorRequest(i, fmt) => {
                    let c = self.theme.index(i, pane.term.lock().colors());
                    pane.write(fmt(alacritty_terminal::vte::ansi::Rgb { r: c.r, g: c.g, b: c.b }).into_bytes());
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
                UiAction::Launcher(id, op) => self.launch_op(id, op),
                UiAction::History(id, op) => self.history_op(id, op),
                UiAction::FocusPane(None, id) => {
                    self.focus_pane(id);
                }
                UiAction::FocusPane(Some(window), id) => self.peers.focus(window, id),
            }
        }
    }

    /// Pane under a screen position.
    fn pane_at(&self, pos: egui::Pos2) -> Option<crate::pane::PaneId> {
        self.rects.iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| *id)
    }

    /// Paths dropped somewhere in the window.
    /// - On the tab bar's "+": one new tab per folder (a file opens its parent folder).
    /// - On a pane at an empty shell prompt, a single folder: `cd` into it. With something
    ///   typed already (say `cp -r `), the path is pasted instead.
    /// - Otherwise: focus the pane and paste the quoted absolute paths (Claude Code turns
    ///   pasted image paths into attachments).
    fn drop_paths(&mut self, paths: &[std::path::PathBuf], pos: egui::Pos2) {
        if self.plus_rect.expand(4.0).contains(pos) {
            for p in paths {
                let dir = if p.is_dir() { Some(p.clone()) } else { p.parent().map(|d| d.to_path_buf()) };
                self.open_tab_at(dir);
            }
            return;
        }
        let Some(id) = self.pane_at(pos) else { return };
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.focus = id;
        }
        if let [dir] = paths {
            if dir.is_dir() && self.panes.get(&id).is_some_and(|p| p.at_empty_prompt()) {
                if let Some(p) = self.panes.get(&id) {
                    let cmd = format!("cd {}\r", crate::sidebar::quote(&dir.to_string_lossy()));
                    p.term.lock().scroll_display(Scroll::Bottom);
                    p.write(cmd.into_bytes());
                }
                return;
            }
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

    /// Adds system fonts for the characters the terminal met that no installed font has.
    fn font_fallbacks(&mut self, ctx: &egui::Context) {
        // egui switches fonts at the start of the frame after `set_fonts`. Only then are the cell
        // size and the cached glyphs out of date.
        if std::mem::take(&mut self.fonts_changed) {
            self.metrics_for = (0.0, 0.0, 0.0);
        }
        for c in self.glyphs.missing.drain(..) {
            self.fallbacks.want(c);
        }
        if self.fallbacks.poll() {
            self.faux_bold = fonts::install(ctx, &self.config.font_family, &self.fallbacks).faux_bold;
            self.fonts_changed = true;
            ctx.request_repaint();
        }
    }

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
        self.font_fallbacks(ctx);
        self.apply_config(ctx);
        self.ensure_metrics(ctx);
        self.process_term_events(ctx);
        self.peer_requests(ctx);
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
            self.sidebar.usage.set_agy_quota(self.config.show_antigravity);
            // Screenshots with `agents` show sample ones instead.
            if !self.devshot.as_ref().is_some_and(|d| d.has("agents")) {
                self.track_agents();
            }
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
        self.sidebar.focused_pane = self.tab().map(|t| t.focus);
        let actions = self.sidebar.show(ctx, &self.theme, &mut self.config, &self.launch_status, &self.hist.store);
        self.run_sidebar_actions(ctx, actions);
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| self.panes_ui(ui));

        let actions = self.sidebar.show_viewer(ctx, &self.theme, self.central);
        self.run_sidebar_actions(ctx, actions);
        self.os_file_drops(ctx);
        self.launch_tick(ctx);
        self.history_tick(ctx);
        self.restore_offer(ctx);
        if self.settings_open {
            self.settings_page(ctx);
        }
        self.ask_tick(ctx);
        self.ask_keep_size(ctx);
        if self.palette.open {
            self.palette_ui(ctx);
        } else {
            self.palette.ask_shown = false;
            self.palette.ask_windowed = false;
        }
        if self.setup.is_some() {
            self.setup_ui(ctx);
        }
        self.zoom_hud(ctx);
        self.window_border(ctx);
        self.update_title(ctx);
        self.devshot(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // The open tabs become the "last session" in the History.
        self.history_exit();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Transparent: frameless windows show the desktop at their rounded corners, and the Ask
        // popup's window (which shares this) is see-through around its glass. The backdrop paints
        // every pixel of the main window anyway.
        [0.0; 4]
    }
}

/// Executable to run for a new window. Linux appends `" (deleted)"` to `/proc/self/exe` once
/// the binary is rebuilt or updated on disk, so strip that suffix to launch the new build; fall
/// back to an absolute `argv[0]`, and finally `/proc/self/exe` itself (which the kernel can still
/// exec even after the file is unlinked).
fn resolve_self_exe(cur: Option<PathBuf>, arg0: Option<std::ffi::OsString>) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    if let Some(p) = cur {
        if p.is_file() {
            return p;
        }
        if let Some(stripped) = p.as_os_str().as_bytes().strip_suffix(b" (deleted)") {
            let orig = PathBuf::from(std::ffi::OsStr::from_bytes(stripped));
            if orig.is_file() {
                return orig;
            }
        }
    }
    if let Some(a) = arg0.map(PathBuf::from).filter(|p| p.is_absolute() && p.is_file()) {
        return a;
    }
    PathBuf::from("/proc/self/exe")
}

#[cfg(test)]
mod tests {
    use super::resolve_self_exe;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn resolve_exe_handles_rebuilt_binary() {
        let dir = std::env::temp_dir().join(format!("s-term-exe-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("s-term");
        let alt = dir.join("s-term-link");
        std::fs::write(&bin, b"").unwrap();
        std::fs::write(&alt, b"").unwrap();

        // Normal: current_exe exists on disk.
        assert_eq!(resolve_self_exe(Some(bin.clone()), None), bin);

        // Rebuilt/updated in place: Linux appends " (deleted)" to /proc/self/exe.
        let deleted = PathBuf::from(format!("{} (deleted)", bin.display()));
        assert_eq!(resolve_self_exe(Some(deleted), None), bin);

        // Removed at original path, but absolute argv[0] still points to an existing binary.
        let gone = PathBuf::from(format!("{} (deleted)", dir.join("missing").display()));
        assert_eq!(resolve_self_exe(Some(gone.clone()), Some(OsString::from(&alt))), alt);

        // Relative argv[0] is ignored (new_window changes current_dir); falls back to /proc/self/exe.
        assert_eq!(resolve_self_exe(Some(gone), Some(OsString::from("./s-term"))), PathBuf::from("/proc/self/exe"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

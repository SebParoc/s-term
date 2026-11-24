//! "Ask S-term": the conversation shown in the search popup, and the tools the assistant uses
//! to see and drive the terminal.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{vec2, FontId, Id, Key, Modifiers, RichText, Sense, Stroke};

use super::STerm;
use crate::assistant::{self, keys, Event, Job, ToolCall, ToolResult, Turn};
use crate::config::{AiProvider, Config};
use crate::icons::{self, Icon};
use crate::layout::Orientation;
use crate::pane::PaneId;
use crate::theme::{c32, Theme};

pub enum ModelList {
    Loading(Receiver<Result<Vec<String>, String>>),
    Ready(Vec<String>),
    Failed(String),
}

/// A command waiting for the user's OK.
struct Pending {
    call: ToolCall,
    reply: Sender<ToolResult>,
    pane: PaneId,
    /// The command (or text), editable before running.
    edit: String,
}

/// A command that was sent to a pane; its output is being collected.
struct Waiting {
    call: ToolCall,
    reply: Sender<ToolResult>,
    pane: PaneId,
    /// `run_command` waits for the shell prompt to come back; `type_text` for output to settle.
    shell: bool,
    started: Instant,
    until: Instant,
    last_text: String,
    last_change: Instant,
    last_check: Instant,
}

#[derive(Default)]
pub struct Ask {
    pub history: Vec<Turn>,
    job: Option<Job>,
    queue: VecDeque<(ToolCall, Sender<ToolResult>)>,
    pending: Option<Pending>,
    waiting: Option<Waiting>,
    pub input: String,
    error: Option<String>,
    pub focus_input: bool,
    pub models: HashMap<AiProvider, ModelList>,
    /// API key being typed, per provider.
    pub key_input: HashMap<AiProvider, String>,
    /// Last answer of `has_key`, so the title bar doesn't read the key file every frame.
    key_check: Option<(AiProvider, Instant, bool)>,
}

impl Ask {
    /// Whether provider `p` has an API key (saved or from the environment). Re-checked every
    /// couple of seconds.
    pub fn has_key(&mut self, p: AiProvider) -> bool {
        match self.key_check {
            Some((cp, at, has)) if cp == p && at.elapsed().as_secs_f32() < 2.0 => has,
            _ => {
                let has = keys::get(p).is_some();
                self.key_check = Some((p, Instant::now(), has));
                has
            }
        }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn needs_attention(&self) -> bool {
        self.pending.is_some()
    }

    /// Starts fetching a provider's model list, if it isn't loaded or loading.
    pub fn ensure_models(&mut self, p: AiProvider, force: bool) {
        if !force && self.models.contains_key(&p) {
            return;
        }
        let Some(key) = keys::get(p) else {
            self.models.remove(&p);
            return;
        };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(assistant::provider::list_models(p, &key).map_err(|e| format!("{e:#}")));
        });
        self.models.insert(p, ModelList::Loading(rx));
    }

    fn poll_models(&mut self) {
        for list in self.models.values_mut() {
            if let ModelList::Loading(rx) = list {
                if let Ok(r) = rx.try_recv() {
                    *list = match r {
                        Ok(v) => ModelList::Ready(v),
                        Err(e) => ModelList::Failed(e),
                    };
                }
            }
        }
    }

    /// Answers every outstanding tool call with "cancelled" and keeps the history valid
    /// (each tool call followed by its result).
    fn cancel_all(&mut self) {
        let cancelled = |c: &ToolCall| ToolResult { id: c.id.clone(), name: c.name.clone(), content: "Cancelled by the user.".into(), is_error: true };
        if let Some(job) = self.job.take() {
            job.cancel();
        }
        if let Some(p) = self.pending.take() {
            let _ = p.reply.send(cancelled(&p.call));
        }
        if let Some(w) = self.waiting.take() {
            let _ = w.reply.send(cancelled(&w.call));
        }
        for (c, r) in self.queue.drain(..) {
            let _ = r.send(cancelled(&c));
        }
        if let Some(Turn::Assistant { calls, .. }) = self.history.last() {
            if !calls.is_empty() {
                let rs = calls.iter().map(cancelled).collect();
                self.history.push(Turn::Results(rs));
            }
        }
    }

    pub fn new_chat(&mut self) {
        self.cancel_all();
        self.history.clear();
        self.error = None;
        self.focus_input = true;
    }
}

fn tilde(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().into_owned();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

fn expand(s: &str) -> PathBuf {
    match (s.strip_prefix('~'), std::env::var_os("HOME")) {
        (Some(rest), Some(h)) => PathBuf::from(h).join(rest.trim_start_matches('/')),
        _ => PathBuf::from(s),
    }
}

impl STerm {
    // ------------------------------------------------------------------------------ terminal facts

    /// Tabs and panes, one line per pane.
    fn pane_list(&self) -> String {
        let mut out = String::new();
        for (ti, tab) in self.tabs.iter().enumerate() {
            let mut ids = Vec::new();
            tab.root.leaves(&mut ids);
            out.push_str(&format!("Tab {}{}:\n", ti + 1, if ti == self.active { " (active)" } else { "" }));
            for id in ids {
                let Some(p) = self.panes.get(&id) else { continue };
                let (fg, idle) = p.foreground();
                let state = if idle { format!("idle at a {fg} prompt") } else { format!("running {fg}") };
                let focused = if ti == self.active && tab.focus == id { " (focused)" } else { "" };
                let cwd = p.cwd().map(|c| tilde(&c)).unwrap_or_else(|| "?".into());
                out.push_str(&format!("  pane {id}{focused}: {cwd}, {state}, title \"{}\", {}x{}\n", p.title, p.size.cols, p.size.rows));
            }
        }
        out
    }

    /// The `<terminal>` block sent with each message.
    fn snapshot(&self) -> String {
        let mut s = String::from("<terminal>\n");
        s.push_str(&self.pane_list());
        if let Some(tab) = self.tab() {
            if let Some(p) = self.panes.get(&tab.focus) {
                s.push_str(&format!("\nLast lines of the focused pane ({}):\n{}\n", tab.focus, p.text_tail(40)));
            }
        }
        s.push_str("</terminal>");
        s
    }

    pub(super) fn focus_pane(&mut self, id: PaneId) -> bool {
        let Some(ti) = self.tabs.iter().position(|t| t.root.contains(id)) else { return false };
        self.active = ti;
        self.tabs[ti].focus = id;
        if self.tabs[ti].zoomed.is_some() {
            self.tabs[ti].zoomed = Some(id);
        }
        true
    }

    // ------------------------------------------------------------------------------------ sending

    pub(super) fn ask_send(&mut self, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() || self.ask.busy() {
            return;
        }
        self.ask.error = None;
        let full = format!("{text}\n\n{}", self.snapshot());
        self.ask.history.push(Turn::User { shown: text, full });
        let p = self.config.ai_provider;
        let model = self.config.ai_model(p).to_string();
        self.ask.job = Some(assistant::start(self.ctx.clone(), p, model, self.ask.history.clone()));
        self.ask.input.clear();
    }

    /// Runs every frame: takes the worker's events, runs tool calls, collects command output.
    pub(super) fn ask_tick(&mut self, ctx: &egui::Context) {
        self.ask.poll_models();
        let mut events = Vec::new();
        if let Some(job) = &self.ask.job {
            while let Ok(e) = job.rx.try_recv() {
                events.push(e);
            }
        }
        for e in events {
            match e {
                Event::Turn(t) => self.ask.history.push(t),
                Event::Tool(call, reply) => self.ask.queue.push_back((call, reply)),
                Event::ModelChosen(p, m) => *self.config.ai_model_mut(p) = m,
                Event::Error(msg) => {
                    // An unanswered message goes back in the input box, to retry or edit.
                    if let Some(Turn::User { shown, .. }) = self.ask.history.last() {
                        if self.ask.input.is_empty() {
                            self.ask.input = shown.clone();
                        }
                        self.ask.history.pop();
                    }
                    self.ask.error = Some(msg);
                }
                Event::Done => {
                    self.ask.job = None;
                    self.ask.focus_input = true;
                }
            }
        }
        self.check_waiting(ctx);
        while self.ask.pending.is_none() && self.ask.waiting.is_none() {
            let Some((call, reply)) = self.ask.queue.pop_front() else { break };
            self.run_tool(call, reply);
        }
        if self.ask.pending.is_some() && !self.palette.open {
            // The assistant needs an OK: bring the conversation back up.
            self.palette.open_ask();
        }
    }

    fn run_tool(&mut self, call: ToolCall, reply: Sender<ToolResult>) {
        let ok = |c: &ToolCall, s: String| ToolResult { id: c.id.clone(), name: c.name.clone(), content: s, is_error: false };
        let err = |c: &ToolCall, s: String| ToolResult { id: c.id.clone(), name: c.name.clone(), content: s, is_error: true };
        let a = call.args.clone();
        let pane_arg = |k: &str| a[k].as_u64();
        let focused = self.tab().map(|t| t.focus);
        let result = match call.name.as_str() {
            "list_panes" => ok(&call, self.pane_list()),
            "read_pane" => match pane_arg("pane_id").and_then(|id| self.panes.get(&id)) {
                Some(p) => {
                    let n = a["lines"].as_u64().unwrap_or(150).clamp(1, 2000) as usize;
                    ok(&call, p.text_tail(n))
                }
                None => err(&call, "No pane with that id. Use list_panes.".into()),
            },
            "open_pane" => {
                let cwd = a["cwd"].as_str().map(expand).filter(|p| p.is_dir()).or_else(|| self.focused_cwd());
                let id = match a["placement"].as_str() {
                    Some("tab") => self.open_tab_at(cwd),
                    Some("down") => self.split_at(Orientation::Stacked, cwd),
                    _ => self.split_at(Orientation::SideBySide, cwd),
                };
                match id {
                    Some(id) => ok(&call, format!("Opened pane {id}. It is now focused.")),
                    None => err(&call, "Couldn't start a shell.".into()),
                }
            }
            "focus_pane" => match pane_arg("pane_id") {
                Some(id) if self.focus_pane(id) => ok(&call, format!("Pane {id} is focused.")),
                _ => err(&call, "No pane with that id.".into()),
            },
            "run_command" | "type_text" | "close_pane" => {
                let pane = pane_arg("pane_id").or(focused);
                let Some(pane) = pane.filter(|id| self.panes.contains_key(id)) else {
                    let _ = reply.send(err(&call, "No pane with that id. Use list_panes.".into()));
                    return;
                };
                if call.name == "run_command" {
                    let (fg, idle) = self.panes[&pane].foreground();
                    if !idle {
                        let _ = reply.send(err(&call, format!("Pane {pane} is busy running {fg}. Open a new pane with open_pane, or use type_text to interact with {fg}.")));
                        return;
                    }
                }
                let edit = match call.name.as_str() {
                    "run_command" => a["command"].as_str().unwrap_or("").to_string(),
                    "type_text" => a["text"].as_str().unwrap_or("").to_string(),
                    _ => String::new(),
                };
                if self.config.ai_auto_run {
                    self.execute(call, reply, pane, edit);
                } else {
                    self.ask.pending = Some(Pending { call, reply, pane, edit });
                }
                return;
            }
            other => err(&call, format!("Unknown tool {other}.")),
        };
        let _ = reply.send(result);
    }

    /// Runs an approved command / text / close.
    fn execute(&mut self, call: ToolCall, reply: Sender<ToolResult>, pane: PaneId, edit: String) {
        let res = |s: String, e: bool| ToolResult { id: call.id.clone(), name: call.name.clone(), content: s, is_error: e };
        let Some(p) = self.panes.get(&pane) else {
            let _ = reply.send(res("That pane was closed.".into(), true));
            return;
        };
        let now = Instant::now();
        match call.name.as_str() {
            "close_pane" => {
                self.close_pane(pane);
                let _ = reply.send(res(format!("Closed pane {pane}."), false));
            }
            "run_command" => {
                p.term.lock().scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                p.write(format!("{}\r", edit.trim_end().replace('\n', "\r")).into_bytes());
                let secs = call.args["wait_seconds"].as_f64().unwrap_or(15.0).clamp(1.0, 120.0);
                let text = p.text_tail(80);
                self.ask.waiting = Some(Waiting { call, reply, pane, shell: true, started: now, until: now + Duration::from_secs_f64(secs), last_text: text, last_change: now, last_check: now });
            }
            _ => {
                let mut bytes = edit.into_bytes();
                if call.args["press_enter"].as_bool() == Some(true) {
                    bytes.push(b'\r');
                }
                p.term.lock().scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                p.write(bytes);
                let text = p.text_tail(80);
                self.ask.waiting = Some(Waiting { call, reply, pane, shell: false, started: now, until: now + Duration::from_secs(8), last_text: text, last_change: now, last_check: now });
            }
        }
    }

    fn check_waiting(&mut self, ctx: &egui::Context) {
        let Some(w) = &mut self.ask.waiting else { return };
        ctx.request_repaint_after(Duration::from_millis(120));
        let now = Instant::now();
        if now - w.last_check < Duration::from_millis(120) {
            return;
        }
        w.last_check = now;
        let Some(p) = self.panes.get(&w.pane) else {
            let w = self.ask.waiting.take().unwrap();
            let _ = w.reply.send(ToolResult { id: w.call.id, name: w.call.name, content: "The pane was closed.".into(), is_error: true });
            return;
        };
        let text = p.text_tail(80);
        if text != w.last_text {
            w.last_text = text;
            w.last_change = now;
        }
        let quiet = now - w.last_change;
        let elapsed = now - w.started;
        let (fg, idle) = p.foreground();
        let finished = if w.shell { elapsed > Duration::from_millis(300) && idle && quiet > Duration::from_millis(250) } else { elapsed > Duration::from_millis(500) && quiet > Duration::from_millis(800) };
        if !finished && now < w.until {
            return;
        }
        let w = self.ask.waiting.take().unwrap();
        let status = match (w.shell, finished) {
            (true, true) => "The command finished.".to_string(),
            (true, false) => format!("Still running after {:.0}s (foreground: {fg}).", elapsed.as_secs_f32()),
            (false, _) => "Typed.".to_string(),
        };
        let content = format!("{status}\n\nLast lines of pane {}:\n{}", w.pane, p.text_tail(60));
        let _ = w.reply.send(ToolResult { id: w.call.id, name: w.call.name, content, is_error: false });
    }

    fn approve(&mut self) {
        if let Some(p) = self.ask.pending.take() {
            self.execute(p.call, p.reply, p.pane, p.edit);
        }
    }

    fn decline(&mut self) {
        if let Some(p) = self.ask.pending.take() {
            let _ = p.reply.send(ToolResult { id: p.call.id, name: p.call.name, content: "The user declined this.".into(), is_error: true });
        }
    }

    fn stop(&mut self) {
        self.ask.cancel_all();
        self.ask.focus_input = true;
    }

    /// Types text at the focused pane's prompt without running it.
    fn insert_text(&mut self, text: &str) {
        self.paste(text.trim_end());
    }

    /// A sample conversation with a command waiting for approval (screenshots only).
    pub(super) fn ask_demo(&mut self) {
        use serde_json::json;
        let pane = self.tab().map(|t| t.focus).unwrap_or(1);
        let read = ToolCall { id: "t1".into(), name: "read_pane".into(), args: json!({"pane_id": pane}) };
        let run = ToolCall { id: "t2".into(), name: "run_command".into(), args: json!({"command": "cargo build --release 2>&1 | tail -20", "pane_id": pane}) };
        self.ask.history = vec![
            Turn::User { shown: "why did my last build fail?".into(), full: String::new() },
            Turn::Assistant { provider: AiProvider::Anthropic, raw: json!([]), text: "Let me look at the pane.".into(), calls: vec![read.clone()] },
            Turn::Results(vec![ToolResult { id: "t1".into(), name: "read_pane".into(), content: String::new(), is_error: false }]),
            Turn::Assistant {
                provider: AiProvider::Anthropic,
                raw: json!([]),
                text: "`ureq` isn't in `Cargo.toml`, so `src/assistant/provider.rs` can't import it. Add it with:\n\n```\ncargo add ureq@2 --features json\n```\n\nThen I'll rebuild to check.".into(),
                calls: vec![run.clone()],
            },
        ];
        let (tx, _rx) = channel();
        self.ask.pending = Some(Pending { call: run, reply: tx, pane, edit: "cargo build --release 2>&1 | tail -20".into() });
    }

    // ------------------------------------------------------------------------------------------ UI

    /// The conversation view inside the popup.
    pub(super) fn ask_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, theme: &Theme, max_h: f32) {
        let p = self.config.ai_provider;
        // An ongoing conversation stays visible even if its key was removed.
        let has_key = keys::get(p).is_some() || !self.ask.history.is_empty();

        // ---- header
        // The whole header row drags the popup (registered first, so its buttons stay on top);
        // double-click puts it back over the middle of S-term.
        let windowed = self.palette.ask_windowed;
        let hdr = egui::Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 28.0));
        let drag = ui.interact(hdr, Id::new("s-term-ask-drag"), Sense::click_and_drag());
        if windowed {
            // The window manager moves the window, so it can go anywhere on the desktop.
            if drag.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        } else if drag.dragged() {
            self.palette.ask_offset += drag.drag_delta();
        }
        if drag.double_clicked() {
            self.palette.recenter_ask();
        }
        if drag.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if drag.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
        // Badge and close button sit in matching 26px boxes, so the row mirrors itself.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (r, _) = ui.allocate_exact_size(vec2(26.0, 28.0), Sense::hover());
            super::chrome::ask_badge(ui.painter(), self.logo.id(), r.center(), 11.0, theme);
            ui.add(egui::Label::new(RichText::new("Ask S-term").size(15.0).strong().color(theme.text)).selectable(false));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                // Same height as the icon buttons, so the model chip lines up with them.
                ui.spacing_mut().interact_size.y = 26.0;
                if icons::button(ui, Icon::Close, 26.0, theme.muted, theme.header_focused, false, Id::new("s-term-ask-close")).on_hover_text("Close (Esc)").clicked() {
                    self.palette.open = false;
                }
                if icons::button(ui, Icon::Plus, 26.0, theme.muted, theme.header_focused, false, Id::new("s-term-ask-new")).on_hover_text("New conversation").clicked() {
                    self.ask.new_chat();
                }
                if icons::button(ui, Icon::Search, 26.0, theme.muted, theme.header_focused, false, Id::new("s-term-ask-cmds")).on_hover_text("Search commands instead").clicked() {
                    self.palette.open_commands();
                }
                ui.add_space(4.0);
                model_picker(ui, theme, &mut self.config, &mut self.ask);
            });
        });
        ui.add_space(4.0);
        ui.separator();

        if !has_key {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Ask S-term needs a {} API key.", p.name())).size(13.0).color(theme.text));
            ui.label(RichText::new("It's stored in ~/.config/s-term/keys.json, readable only by you. Or set the provider's environment variable.").size(11.5).color(theme.muted));
            ui.add_space(8.0);
            key_editor(ui, theme, &mut self.config, &mut self.ask);
            ui.add_space(6.0);
            return;
        }

        // ---- conversation
        let mut insert: Option<String> = None;
        let shown_any = !self.ask.history.is_empty() || self.ask.error.is_some() || self.ask.busy();
        if shown_any {
            egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).stick_to_bottom(true).show(ui, |ui| {
                ui.add_space(6.0);
                let results: HashMap<&str, &ToolResult> = self
                    .ask
                    .history
                    .iter()
                    .filter_map(|t| if let Turn::Results(rs) = t { Some(rs.iter()) } else { None })
                    .flatten()
                    .map(|r| (r.id.as_str(), r))
                    .collect();
                // Consecutive assistant turns (text, tool calls, more text) share one bubble.
                let thinking = (self.ask.busy() && self.ask.pending.is_none())
                    .then(|| if self.ask.waiting.is_some() { "Waiting for the command…" } else { "Thinking…" });
                let hist = &self.ask.history;
                let mut i = 0;
                while i < hist.len() {
                    if let Turn::User { shown, .. } = &hist[i] {
                        user_bubble(ui, theme, shown);
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < hist.len() && !matches!(hist[i], Turn::User { .. }) {
                        i += 1;
                    }
                    let group = &hist[start..i];
                    let visible = group.iter().any(|t| matches!(t, Turn::Assistant { text, calls, .. } if !text.trim().is_empty() || !calls.is_empty()));
                    let live = i == hist.len() && thinking.is_some();
                    if !visible && !live {
                        continue;
                    }
                    assistant_bubble(ui, theme, self.logo.id(), |ui| {
                        for t in group {
                            if let Turn::Assistant { text, calls, .. } = t {
                                if !text.trim().is_empty() {
                                    if let Some(t) = markdown(ui, theme, text) {
                                        insert = Some(t);
                                    }
                                }
                                for c in calls {
                                    tool_row(ui, theme, c, results.get(c.id.as_str()).copied());
                                }
                            }
                        }
                        if live {
                            working(ui, theme, thinking.unwrap_or_default());
                        }
                    });
                }
                if let (Some(label), true) = (thinking, matches!(hist.last(), Some(Turn::User { .. }) | None)) {
                    assistant_bubble(ui, theme, self.logo.id(), |ui| working(ui, theme, label));
                }
                if let Some(e) = &self.ask.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).size(12.0).color(c32(theme.palette[1])));
                }
                ui.add_space(4.0);
            });
        }

        // ---- approval
        let mut decision: Option<bool> = None;
        if let Some(pend) = &mut self.ask.pending {
            ui.add_space(6.0);
            let cwd = self.panes.get(&pend.pane).and_then(|p| p.cwd()).map(|c| tilde(&c)).unwrap_or_default();
            egui::Frame::none().fill(theme.bar).stroke(Stroke::new(1.0_f32, theme.accent)).rounding(12.0).inner_margin(10.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let what = match pend.call.name.as_str() {
                    "run_command" => format!("Run in pane {} · {cwd}", pend.pane),
                    "type_text" => format!("Type into pane {}", pend.pane),
                    _ => format!("Close pane {} · {cwd}", pend.pane),
                };
                ui.label(RichText::new(what).size(11.5).color(theme.muted));
                if pend.call.name != "close_pane" {
                    ui.add(egui::TextEdit::multiline(&mut pend.edit).font(FontId::monospace(13.0)).desired_rows(1).desired_width(f32::INFINITY).frame(false));
                }
                ui.horizontal(|ui| {
                    let run = if pend.call.name == "close_pane" { "Close ⏎" } else { "Run ⏎" };
                    if ui.add(egui::Button::new(RichText::new(run).color(theme.pick_ink)).fill(theme.accent)).clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Skip").clicked() {
                        decision = Some(false);
                    }
                    ui.label(RichText::new("Settings → Assistant can run commands without asking.").size(10.5).color(theme.muted));
                });
            });
            let (enter, ctrl_enter) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Enter), i.consume_key(Modifiers::CTRL, Key::Enter)));
            if enter || ctrl_enter {
                decision = Some(true);
            }
        }
        match decision {
            Some(true) => self.approve(),
            Some(false) => self.decline(),
            None => {}
        }
        if let Some(t) = insert {
            self.insert_text(&t);
            self.palette.open = false;
        }

        // ---- input
        ui.add_space(6.0);
        let mut send = false;
        let input_id = Id::new("s-term-ask-input");
        let focused = ctx.memory(|m| m.has_focus(input_id));
        let rim = if focused { theme.accent.gamma_multiply(0.7) } else { theme.border };
        egui::Frame::none()
            .fill(theme.bar)
            .stroke(Stroke::new(1.0_f32, rim))
            .rounding(14.0)
            .inner_margin(egui::Margin { left: 12.0, right: 5.0, top: 5.0, bottom: 5.0 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let busy = self.ask.busy();
                    let hint = if self.ask.history.is_empty() { "Ask anything, or tell S-term what to do…" } else { "Ask a follow-up…" };
                    let w = ui.available_width() - 72.0;
                    let edit = egui::TextEdit::singleline(&mut self.ask.input)
                        .id(input_id)
                        .hint_text(RichText::new(hint).color(theme.muted))
                        .font(FontId::proportional(14.0))
                        .text_color(theme.text)
                        .frame(false)
                        .margin(vec2(0.0, 4.0))
                        .desired_width(w);
                    let resp = ui.add_enabled(self.ask.pending.is_none(), edit);
                    if std::mem::take(&mut self.ask.focus_input) && self.ask.pending.is_none() {
                        resp.request_focus();
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !busy {
                        send = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (label, fill, ink, enabled) = if busy {
                            ("Stop", theme.header_focused, theme.text, true)
                        } else {
                            let ok = !self.ask.input.trim().is_empty();
                            ("Send", if ok { theme.accent } else { theme.header }, if ok { theme.pick_ink } else { theme.muted }, ok)
                        };
                        let btn = egui::Button::new(RichText::new(label).size(13.0).color(ink)).fill(fill).stroke(Stroke::NONE).rounding(10.0).min_size(vec2(60.0, 28.0));
                        if ui.add_enabled(enabled, btn).clicked() {
                            if busy {
                                self.stop();
                            } else {
                                send = true;
                            }
                        }
                    });
                });
            });
        if send {
            let text = std::mem::take(&mut self.ask.input);
            self.ask_send(text);
            self.ask.focus_input = true;
        }
        ui.add_space(4.0);
        let mode = if self.config.ai_auto_run { "commands run without asking" } else { "you approve each command" };
        ui.label(RichText::new(format!("⏎ send   esc close   ·   {mode}")).size(10.5).color(theme.muted));
    }
}

fn user_bubble(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.add_space(4.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        let r = egui::Rounding { nw: 14.0, ne: 14.0, sw: 14.0, se: 4.0 };
        egui::Frame::none()
            .fill(theme.accent.gamma_multiply(0.22))
            .stroke(Stroke::new(1.0_f32, theme.accent.gamma_multiply(0.35)))
            .rounding(r)
            .inner_margin(egui::Margin::symmetric(12.0, 7.0))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() * 0.78);
                ui.label(RichText::new(text).size(13.5).color(theme.text));
            });
    });
    ui.add_space(8.0);
}

/// The assistant's reply: a small S-term badge, then a soft card holding its text and tool calls.
fn assistant_bubble(ui: &mut egui::Ui, theme: &Theme, logo: egui::TextureId, body: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(vec2(22.0, 24.0), Sense::hover());
        super::chrome::ask_badge(ui.painter(), logo, egui::pos2(r.center().x, r.top() + 12.0), 9.0, theme);
        let max_w = (ui.available_width() - 24.0).max(120.0);
        egui::Frame::none()
            .fill(theme.header)
            .stroke(Stroke::new(1.0_f32, theme.border))
            .rounding(egui::Rounding { nw: 4.0, ne: 14.0, sw: 14.0, se: 14.0 })
            .inner_margin(egui::Margin::symmetric(12.0, 9.0))
            .show(ui, |ui| {
                ui.set_max_width(max_w);
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.vertical(body);
            });
    });
    ui.add_space(8.0);
}

/// One line of prose with `inline code` and **bold** spans styled.
fn rich_line(ui: &mut egui::Ui, theme: &Theme, text: &str, size: f32, base: egui::Color32) {
    use egui::text::{LayoutJob, TextFormat};
    let mut job = LayoutJob::default();
    let prose = FontId::proportional(size);
    let mono = FontId::monospace(size - 1.0);
    for (i, seg) in text.split('`').enumerate() {
        if seg.is_empty() {
            continue;
        }
        if i % 2 == 1 {
            let fmt = TextFormat { font_id: mono.clone(), color: theme.accent2, background: theme.bar, ..Default::default() };
            job.append(&format!("\u{2009}{seg}\u{2009}"), 0.0, fmt);
        } else {
            for (j, part) in seg.split("**").enumerate() {
                if part.is_empty() {
                    continue;
                }
                let color = if j % 2 == 1 { egui::Color32::WHITE.lerp_to_gamma(theme.text, 0.3) } else { base };
                job.append(part, 0.0, TextFormat { font_id: prose.clone(), color, ..Default::default() });
            }
        }
    }
    job.wrap.max_width = ui.available_width();
    ui.add(egui::Label::new(job).wrap());
}

fn working(ui: &mut egui::Ui, theme: &Theme, label: &str) {
    let t = ui.input(|i| i.time);
    let dots = ".".repeat(1 + (t * 3.0) as usize % 3);
    ui.ctx().request_repaint_after(Duration::from_millis(150));
    ui.label(RichText::new(format!("{}{dots}", label.trim_end_matches('…'))).size(12.5).italics().color(theme.muted));
}

fn tool_row(ui: &mut egui::Ui, theme: &Theme, call: &ToolCall, result: Option<&ToolResult>) {
    let (mark, col) = match result {
        None => ("…", theme.muted),
        Some(r) if r.content.starts_with("The user declined") => ("declined", c32(theme.palette[3])),
        Some(r) if r.content.starts_with("Cancelled") => ("cancelled", theme.muted),
        Some(r) if r.is_error => ("failed", c32(theme.palette[1])),
        Some(_) => ("✓", c32(theme.palette[2])),
    };
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
        icons::draw(ui.painter(), Icon::Terminal, icons::center_rect(r.center(), 12.0), theme.muted);
        let resp = ui.label(RichText::new(assistant::describe(call)).size(12.0).color(theme.text.gamma_multiply(0.8)));
        ui.label(RichText::new(mark).size(11.0).color(col));
        if let Some(r) = result.filter(|r| r.is_error) {
            resp.on_hover_text(&r.content);
        }
    });
}

/// A small markdown subset: paragraphs, `#` headings, bullets and fenced code blocks (with Copy
/// and Insert buttons). Returns text to insert at the prompt if Insert was clicked.
fn markdown(ui: &mut egui::Ui, theme: &Theme, text: &str) -> Option<String> {
    let mut insert = None;
    let mut in_code = false;
    let mut code = String::new();
    let mut para = String::new();
    let flush = |ui: &mut egui::Ui, para: &mut String| {
        if !para.trim().is_empty() {
            for l in para.trim_end().lines() {
                rich_line(ui, theme, l, 13.5, theme.text);
            }
            ui.add_space(3.0);
        }
        para.clear();
    };
    let code_block = |ui: &mut egui::Ui, code: &str, insert: &mut Option<String>| {
        egui::Frame::none().fill(theme.bar).stroke(Stroke::new(1.0_f32, theme.border)).rounding(8.0).inner_margin(egui::Margin::symmetric(10.0, 8.0)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(code.trim_end()).monospace().size(12.5).color(theme.accent2));
            ui.horizontal(|ui| {
                if ui.small_button("Copy").clicked() {
                    ui.ctx().output_mut(|o| o.copied_text = code.trim_end().to_string());
                }
                if ui.small_button("Insert at prompt").on_hover_text("Type it into the focused pane without running it").clicked() {
                    *insert = Some(code.trim_end().to_string());
                }
            });
        });
        ui.add_space(4.0);
    };
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if in_code {
                code_block(ui, &code, &mut insert);
                code.clear();
            } else {
                flush(ui, &mut para);
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            code.push_str(line);
            code.push('\n');
        } else if let Some(h) = line.strip_prefix('#') {
            flush(ui, &mut para);
            ui.add_space(2.0);
            ui.label(RichText::new(h.trim_start_matches('#').trim().replace("**", "")).size(14.5).strong().color(theme.text));
        } else if line.trim().is_empty() {
            flush(ui, &mut para);
        } else {
            let l = line.trim_start();
            let l = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")).map(|b| format!("•  {b}")).unwrap_or_else(|| line.to_string());
            para.push_str(&l);
            para.push('\n');
        }
    }
    if in_code && !code.is_empty() {
        code_block(ui, &code, &mut insert);
    }
    flush(ui, &mut para);
    insert
}

/// Provider · model chip, with every model the saved keys can use.
pub(super) fn model_picker(ui: &mut egui::Ui, theme: &Theme, cfg: &mut Config, ask: &mut Ask) {
    let p = cfg.ai_provider;
    let m = cfg.ai_model(p);
    let label = if m.is_empty() { format!("{} · auto", p.name()) } else { format!("{} · {}", p.name(), crate::usage_ui::model_name(m)) };
    let resp = egui::ComboBox::from_id_salt("s-term-ask-model").selected_text(RichText::new(label).size(11.5).color(theme.muted)).width(210.0).height(360.0).show_ui(ui, |ui| {
        for prov in AiProvider::ALL {
            ui.label(RichText::new(prov.name()).size(10.5).strong().color(theme.muted));
            if keys::get(prov).is_none() {
                ui.label(RichText::new("  Add a key in Settings → Assistant").size(11.0).color(theme.muted));
                continue;
            }
            ask.ensure_models(prov, false);
            let current = cfg.ai_model(prov).to_string();
            let mut list: Vec<String> = match ask.models.get(&prov) {
                Some(ModelList::Ready(v)) => v.clone(),
                Some(ModelList::Loading(_)) => {
                    ui.label(RichText::new("  Loading models…").size(11.0).color(theme.muted));
                    Vec::new()
                }
                Some(ModelList::Failed(e)) => {
                    ui.label(RichText::new(format!("  {e}")).size(11.0).color(c32(theme.palette[1])));
                    Vec::new()
                }
                None => Vec::new(),
            };
            if !current.is_empty() && !list.contains(&current) {
                list.insert(0, current.clone());
            }
            for id in list {
                let on = cfg.ai_provider == prov && current == id;
                if ui.selectable_label(on, crate::usage_ui::model_name(&id)).on_hover_text(&id).clicked() {
                    cfg.ai_provider = prov;
                    *cfg.ai_model_mut(prov) = id;
                }
            }
        }
    });
    resp.response.on_hover_text("Provider and model");
}

/// Provider picker plus the API key field for it.
pub(super) fn key_editor(ui: &mut egui::Ui, theme: &Theme, cfg: &mut Config, ask: &mut Ask) {
    provider_picker(ui, theme, cfg);
    ui.add_space(12.0);
    ui.label(RichText::new(key_status(cfg.ai_provider)).size(11.5).color(theme.muted));
    ui.add_space(6.0);
    key_field(ui, theme, cfg, ask);
}

/// Segmented Anthropic / OpenAI / Gemini picker; a dot marks providers with a key.
pub(super) fn provider_picker(ui: &mut egui::Ui, theme: &Theme, cfg: &mut Config) {
    let labels: Vec<(AiProvider, String)> = AiProvider::ALL
        .iter()
        .map(|&p| (p, if keys::get(p).is_some() { format!("{}  ●", p.name()) } else { p.name().to_string() }))
        .collect();
    let opts: Vec<(AiProvider, &str)> = labels.iter().map(|(p, l)| (*p, l.as_str())).collect();
    super::settings::segmented(ui, theme, &mut cfg.ai_provider, &opts, "seg-provider");
}

pub(super) fn key_status(p: AiProvider) -> String {
    match keys::source(p) {
        keys::Source::Saved => format!("Saved key {}", keys::get(p).map(|k| keys::masked(&k)).unwrap_or_default()),
        keys::Source::Env(v) => format!("Using ${v}"),
        keys::Source::Missing => format!("No key yet. You can also set ${}.", keys::env_vars(p)[0]),
    }
}

/// Key input with Save / Remove, all one height, filling the available width.
pub(super) fn key_field(ui: &mut egui::Ui, theme: &Theme, cfg: &mut Config, ask: &mut Ask) {
    const H: f32 = 30.0;
    const BTN: f32 = 84.0;
    let p = cfg.ai_provider;
    let saved = keys::source(p) == keys::Source::Saved;
    let buf = ask.key_input.entry(p).or_default();
    let mut save = false;
    let mut forget = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let hint = match p {
            AiProvider::Anthropic => "Paste a new key (sk-ant-…)",
            AiProvider::OpenAi => "Paste a new key (sk-…)",
            AiProvider::Gemini => "Paste a new key (AIza…)",
        };
        let buttons = if saved { 2.0 * BTN + 16.0 } else { BTN + 8.0 };
        let w = (ui.available_width() - buttons).max(160.0);
        let r = ui.add_sized(
            vec2(w, H),
            egui::TextEdit::singleline(buf).password(true).hint_text(hint).margin(vec2(10.0, 7.0)).vertical_align(egui::Align::Center),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !buf.trim().is_empty() {
            save = true;
        }
        let can_save = !buf.trim().is_empty();
        let save_btn = if can_save {
            egui::Button::new(RichText::new("Save key").color(theme.pick_ink)).fill(theme.accent)
        } else {
            egui::Button::new("Save key")
        };
        if ui.add_enabled(can_save, save_btn.min_size(vec2(BTN, H))).clicked() {
            save = true;
        }
        if saved && ui.add(egui::Button::new("Remove").min_size(vec2(BTN, H))).clicked() {
            forget = true;
        }
    });
    if save || forget {
        let key = if forget { String::new() } else { std::mem::take(buf) };
        if let Err(e) = keys::set(p, &key) {
            eprintln!("s-term: saving API key: {e}");
        }
        ask.models.remove(&p);
        ask.ensure_models(p, true);
        ask.focus_input = true;
    }
    if let Some(ModelList::Failed(e)) = ask.models.get(&p) {
        ui.add_space(4.0);
        ui.label(RichText::new(e).size(11.0).color(c32(theme.palette[1])));
    }
}

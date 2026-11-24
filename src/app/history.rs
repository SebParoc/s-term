//! The History (see `crate::history`): records tabs and panes as they close, keeps this window's
//! session file, offers the last session at startup, and reopens entries.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Scroll;
use egui::{pos2, Align2, Id, RichText};

use super::{STerm, Tab, CLICK};
use crate::history::{self as hist, Entry, History, Kind, Layout, Op, PaneRec, TabRec};
use crate::icons::{self, Icon};
use crate::layout::Orientation;
use crate::pane::PaneId;

/// How often open panes are read (folder, program, agent conversation) for the session file.
const PROBE_EVERY: Duration = Duration::from_secs(2);
/// A shell whose prompt the heuristic doesn't recognize counts as ready after this long.
const PROMPT_GRACE: Duration = Duration::from_millis(1500);
/// A line still not typed by then is dropped (the pane never got to a prompt).
const QUEUE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(super) struct HistState {
    pub(super) store: History,
    /// Each open pane as last read. A pane whose shell has exited can't be read any more, so its
    /// entry is made from this.
    live: HashMap<PaneId, PaneRec>,
    probed: Option<Instant>,
    /// The session the startup popup offers (an entry id).
    offer: Option<u64>,
    /// The window's first pane, until something is typed into it: reopening a session replaces it.
    pristine: Option<PaneId>,
    /// Lines to type into reopened panes once their prompt is up: pane, line, run it, since when.
    queued: Vec<(PaneId, String, bool, Instant)>,
}

impl HistState {
    /// Something was typed into a pane: the popup goes, and the first pane is in use.
    pub(super) fn typed(&mut self) {
        self.offer = None;
        self.pristine = None;
    }
}

impl STerm {
    /// At startup, once the first tab is open: loads the history, turns the sessions of windows
    /// that are gone into entries, and reopens or offers the last session.
    pub(super) fn history_start(&mut self) {
        self.hist.pristine = self.tab().map(|t| t.focus);
        // Screenshots keep the history in memory: they never touch the user's files.
        if self.devshot.is_some() {
            return;
        }
        self.hist.store = History::open(hist::default_dir());
        if !self.config.keep_history {
            return;
        }
        self.hist.store.adopt_orphans();
        // Another window is open: this one is a new window, not a fresh start.
        if self.hist.store.others_running() {
            return;
        }
        let Some(id) = self.hist.store.offer().map(|e| e.id) else { return };
        if self.config.restore_session {
            self.restore(id, false);
        } else {
            self.hist.offer = Some(id);
            self.hist.store.mark_offered(id);
        }
    }

    /// A pane as it is now, or as last read when it can't be read any more (its shell has exited).
    fn pane_rec(&self, id: PaneId) -> PaneRec {
        self.read_pane(id).or_else(|| self.hist.live.get(&id).cloned()).unwrap_or_default()
    }

    fn read_pane(&self, id: PaneId) -> Option<PaneRec> {
        let p = self.panes.get(&id)?;
        let cwd = Some(p.cwd()?);
        let (command, agent) = p.foreground_pgid().map(hist::probe).unwrap_or_default();
        Some(PaneRec { cwd, title: p.title.clone(), command, agent })
    }

    fn tab_rec(&self, tab: &Tab, fresh: bool) -> TabRec {
        let mut leaves = Vec::new();
        tab.root.leaves(&mut leaves);
        let mut rec = |id: PaneId| if fresh { self.pane_rec(id) } else { self.hist.live.get(&id).cloned().unwrap_or_default() };
        TabRec { layout: hist::layout_of(&tab.root, &mut rec), focus: leaves.iter().position(|&l| l == tab.focus).unwrap_or(0) }
    }

    /// Just before pane `id` closes: keeps it in the History, or its whole tab when it's the last
    /// pane there.
    pub(super) fn remember_pane(&mut self, id: PaneId) {
        if !self.config.keep_history || !self.panes.contains_key(&id) {
            return;
        }
        let Some(tab) = self.tabs.iter().find(|t| t.root.contains(id)) else { return };
        let entry = match tab.root {
            crate::layout::Node::Leaf(_) => Entry { kind: Kind::Tab, closed: hist::now(), tabs: vec![self.tab_rec(tab, true)], ..Default::default() },
            _ => Entry { kind: Kind::Pane, closed: hist::now(), tabs: vec![TabRec { layout: Layout::Pane(self.pane_rec(id)), focus: 0 }], ..Default::default() },
        };
        self.hist.store.push(entry);
    }

    /// Just before tab `ti` closes: keeps it in the History.
    pub(super) fn remember_tab(&mut self, ti: usize) {
        let Some(tab) = self.tabs.get(ti).filter(|_| self.config.keep_history) else { return };
        let entry = Entry { kind: Kind::Tab, closed: hist::now(), tabs: vec![self.tab_rec(tab, true)], ..Default::default() };
        self.hist.store.push(entry);
    }

    /// The window is closing: its tabs become a session entry.
    pub(super) fn history_exit(&mut self) {
        if self.devshot.is_some() {
            return;
        }
        let tabs = if self.config.keep_history { self.tabs.iter().map(|t| self.tab_rec(t, true)).collect() } else { Vec::new() };
        self.hist.store.end_session(tabs, self.active);
    }

    /// Every frame: types queued lines, picks up other windows' entries, and every couple of
    /// seconds reads the open panes and updates this window's session file.
    pub(super) fn history_tick(&mut self, ctx: &egui::Context) {
        self.type_queued(ctx);
        self.hist.store.refresh();
        if self.hist.probed.is_some_and(|t| t.elapsed() < PROBE_EVERY) {
            return;
        }
        self.hist.probed = Some(Instant::now());
        // Read again on the next round even when nothing else repaints.
        ctx.request_repaint_after(PROBE_EVERY);
        let read: Vec<(PaneId, PaneRec)> = self.panes.keys().filter_map(|&id| Some((id, self.read_pane(id)?))).collect();
        let panes = &self.panes;
        self.hist.live.retain(|id, _| panes.contains_key(id));
        self.hist.live.extend(read);
        if self.config.keep_history && self.devshot.is_none() {
            let tabs: Vec<TabRec> = self.tabs.iter().map(|t| self.tab_rec(t, false)).collect();
            self.hist.store.write_session(&tabs, self.active);
        }
    }

    /// Reopens a History entry. Its tabs come back as new tabs (a single pane can come back as a
    /// split of the focused pane instead), each pane in its folder. Agent conversations resume;
    /// any other program is typed at the prompt, ready to run again.
    pub(super) fn restore(&mut self, id: u64, as_split: bool) {
        let Some(e) = self.hist.store.take(id) else { return };
        self.hist.offer = None;
        let folder = |p: &PaneRec| p.cwd.clone().filter(|d| d.is_dir());
        if as_split && !self.tabs.is_empty() {
            if let [p] = e.panes().as_slice() {
                let p = (*p).clone();
                if let Some(pane) = self.split_at(Orientation::SideBySide, folder(&p)) {
                    self.queue_line(pane, &p);
                }
                return;
            }
        }
        // The untouched first pane of a fresh window makes way for what's reopened.
        let replace = self.hist.pristine.take().filter(|p| self.tabs.len() == 1 && matches!(self.tabs[0].root, crate::layout::Node::Leaf(x) if x == *p));
        let first = self.tabs.len();
        for t in &e.tabs {
            let mut started = Vec::new();
            let root = hist::build(&t.layout, &mut |p| {
                let id = self.spawn_pane(folder(p))?;
                started.push((id, p.clone()));
                Some(id)
            });
            let Some(root) = root else { continue };
            let mut leaves = Vec::new();
            root.leaves(&mut leaves);
            let focus = leaves.get(t.focus).copied().unwrap_or(leaves[0]);
            self.tabs.push(Tab { root, focus, zoomed: None, broadcast: false });
            for (pane, p) in started {
                self.queue_line(pane, &p);
            }
        }
        if self.tabs.len() == first {
            return;
        }
        self.active = first + e.active.min(self.tabs.len() - first - 1);
        if let Some(p) = replace {
            self.drop_pane(p);
        }
    }

    /// Ctrl+Shift+R: reopens the newest entry.
    pub(super) fn reopen_last(&mut self) {
        self.hist.store.refresh();
        if let Some(id) = self.hist.store.entries.first().map(|e| e.id) {
            self.restore(id, false);
        }
    }

    pub(super) fn history_op(&mut self, id: u64, op: Op) {
        match op {
            Op::Restore => self.restore(id, false),
            Op::Split => self.restore(id, true),
            Op::Delete => {
                self.hist.store.take(id);
                if self.hist.offer == Some(id) {
                    self.hist.offer = None;
                }
            }
            Op::Clear => {
                self.hist.store.clear();
                self.hist.offer = None;
            }
            Op::Launcher => {
                let dir = self.hist.store.get(id).and_then(|e| e.panes().first().and_then(|p| p.cwd.clone()));
                self.sidebar.new_launcher(dir);
            }
        }
    }

    fn queue_line(&mut self, pane: PaneId, p: &PaneRec) {
        if let Some((line, run)) = hist::restore_line(p) {
            self.hist.queued.push((pane, line, run, Instant::now()));
        }
    }

    fn type_queued(&mut self, ctx: &egui::Context) {
        if self.hist.queued.is_empty() {
            return;
        }
        let now = Instant::now();
        let panes = &self.panes;
        self.hist.queued.retain(|(id, line, run, since)| {
            let Some(p) = panes.get(id) else { return false };
            let waited = now - *since;
            if p.foreground().1 && (waited > PROMPT_GRACE || p.at_empty_prompt()) {
                p.term.lock().scroll_display(Scroll::Bottom);
                p.write(format!("{line}{}", if *run { "\r" } else { "" }).into_bytes());
                return false;
            }
            waited < QUEUE_TIMEOUT
        });
        if !self.hist.queued.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// The popup at the bottom of the terminal that offers to reopen the last session. It goes
    /// away once something is typed into a pane.
    pub(super) fn restore_offer(&mut self, ctx: &egui::Context) {
        let Some(id) = self.hist.offer else { return };
        let Some(e) = self.hist.store.get(id) else {
            self.hist.offer = None;
            return;
        };
        if self.modal_open() || self.central == egui::Rect::NOTHING {
            return;
        }
        let theme = &self.theme;
        let n = e.tabs.len();
        let detail = format!("{n} tab{} · {}", if n == 1 { "" } else { "s" }, e.describe().1);
        let (mut restore, mut dismiss) = (false, false);
        egui::Area::new(Id::new("s-term-restore-offer"))
            .order(egui::Order::Foreground)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(self.central.center().x, self.central.max.y - 18.0))
            .show(ctx, |ui| {
                crate::glass::frame(ui, theme, 16.0, theme.elevated, crate::glass::Lift::High, egui::Margin { left: 14.0, right: 10.0, top: 10.0, bottom: 10.0 }, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 15.0, theme.accent.gamma_multiply(0.16));
                        icons::draw(ui.painter(), Icon::History, icons::center_rect(r.center(), 15.0), theme.accent);
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Reopen your last session?").size(13.0).strong().color(theme.text));
                            ui.label(RichText::new(detail).size(11.0).color(theme.muted));
                        });
                        ui.add_space(10.0);
                        let btn = egui::Button::new(RichText::new("Reopen").color(theme.bar)).fill(theme.accent).sense(CLICK);
                        restore = ui.add(btn).on_hover_text("Reopen its tabs (Ctrl+Shift+R)").clicked();
                        dismiss = icons::button(ui, Icon::Close, 22.0, theme.muted, theme.header, false, Id::new("s-term-restore-offer-x")).on_hover_text("Not now (it stays in History)").clicked();
                    });
                });
            });
        if restore {
            self.restore(id, false);
        } else if dismiss {
            self.hist.offer = None;
        }
    }

    /// Screenshot mode: sample entries (kept in memory), and the popup with `offer`.
    pub(super) fn history_demo(&mut self, offer: bool) {
        let here = std::env::current_dir().ok();
        let dir = |sub: &str| here.as_ref().map(|h| h.join(sub)).or_else(|| here.clone());
        let pane = |cwd: Option<std::path::PathBuf>, title: &str, command: &[&str]| PaneRec { cwd, title: title.into(), command: command.iter().map(|s| s.to_string()).collect(), agent: None };
        let mut claude = pane(here.clone(), "✳ Add a History panel", &["claude"]);
        claude.agent = Some(hist::Agent { kind: hist::AgentKind::Claude, session: "08dbf0dd-5f37-4bb8-b4ed-dc8ec1731037".into() });
        let tab = |layout| TabRec { layout, focus: 0 };
        let split = |a, b| Layout::Split { side_by_side: true, ratio: 0.5, a: Box::new(Layout::Pane(a)), b: Box::new(Layout::Pane(b)) };
        let now = hist::now();
        let entries = [
            (Kind::Session, now - 26 * 3600, vec![tab(split(pane(here.clone(), "", &[]), pane(dir("src"), "", &["cargo", "watch"]))), tab(Layout::Pane(pane(dir("scripts"), "", &[])))]),
            (Kind::Tab, now - 3 * 3600, vec![tab(split(pane(here.clone(), "", &["npm", "run", "dev"]), pane(dir("assets"), "", &[])))]),
            (Kind::Pane, now - 40 * 60, vec![tab(Layout::Pane(pane(dir("src"), "", &["cargo", "test"])))]),
            (Kind::Tab, now - 5 * 60, vec![tab(Layout::Pane(claude))]),
        ];
        for (kind, closed, tabs) in entries {
            self.hist.store.push(Entry { kind, closed, tabs, active: 0, ..Default::default() });
        }
        if offer {
            let tabs = vec![tab(Layout::Pane(pane(here.clone(), "", &[]))), tab(Layout::Pane(pane(dir("src"), "", &[]))), tab(Layout::Pane(pane(dir("scripts"), "", &[])))];
            self.hist.store.push(Entry { kind: Kind::Session, closed: now, tabs, ..Default::default() });
            self.hist.offer = self.hist.store.offer().map(|e| e.id);
        }
    }
}

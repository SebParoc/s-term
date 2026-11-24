//! Runs the Launchpad's launchers (see `crate::launchpad`): opens a launcher's pane, waits for the
//! shell's prompt, types its command line, and keeps track of what the pane is doing.

use std::time::{Duration, Instant};

use alacritty_terminal::grid::Scroll;

use super::STerm;
use crate::config::RunIn;
use crate::launchpad::{self, Op, State, Status};
use crate::layout::Orientation;
use crate::pane::PaneId;

/// A shell whose prompt the heuristic doesn't recognize counts as ready after this long.
const PROMPT_GRACE: Duration = Duration::from_millis(1500);
/// Drop a queued command line if the pane still isn't at a prompt by then (say, Ctrl+C was ignored).
const QUEUE_TIMEOUT: Duration = Duration::from_secs(10);
/// How often statuses are re-read (each read goes to /proc).
const STATUS_EVERY: Duration = Duration::from_millis(250);

/// A launcher's pane, and the command line waiting to be typed into it.
pub(super) struct Run {
    pane: PaneId,
    queued: Option<(String, Instant)>,
    /// When the line was typed. The shell still looks idle for a moment before it starts the program.
    sent: Option<Instant>,
    /// The typed line hasn't finished. Only then is the pane the launcher's: afterwards it's just a
    /// tab that may run anything, which Stop must not Ctrl+C.
    active: bool,
    /// The program last seen in the foreground.
    fg: String,
    /// Status reads in a row that found the shell idle.
    idle_reads: u8,
}

impl Run {
    fn new(pane: PaneId, line: String) -> Self {
        Self { pane, queued: Some((line, Instant::now())), sent: None, active: false, fg: String::new(), idle_reads: 0 }
    }
}

impl STerm {
    pub(super) fn launch_op(&mut self, id: u64, op: Op) {
        self.launch_checked = None;
        if op == Op::Run {
            return self.launch(id);
        }
        let Some(pane) = self.launches.get(&id).filter(|r| r.active || r.queued.is_some()).map(|r| r.pane).filter(|p| self.panes.contains_key(p)) else { return };
        match op {
            Op::Stop | Op::Restart => {
                self.panes[&pane].write(vec![0x03]);
                let line = self.config.launchers.iter().find(|l| l.id == id).map(|l| launchpad::command_line(l, true));
                if let Some(run) = self.launches.get_mut(&id) {
                    // Restart: typed once the program has quit and the prompt is back.
                    run.queued = line.filter(|_| op == Op::Restart).map(|l| (l, Instant::now()));
                }
            }
            Op::Show => {
                self.focus_pane(pane);
            }
            Op::Close => self.close_pane(pane),
            Op::Run => {}
        }
    }

    fn launch(&mut self, id: u64) {
        let Some(l) = self.config.launchers.iter().find(|l| l.id == id).cloned() else { return };
        if launchpad::steps(&l).is_empty() {
            return;
        }
        // Its pane from last time: still running, so just bring it up; or back at a prompt, so run
        // again there. If it's busy with something else now, leave it be and start a new one.
        if let Some(run) = self.launches.get_mut(&id).filter(|r| self.panes.contains_key(&r.pane)) {
            let pane = run.pane;
            let busy = run.active || run.queued.is_some();
            let idle = self.panes[&pane].foreground().1;
            if !busy && idle {
                run.queued = Some((launchpad::command_line(&l, true), Instant::now()));
            }
            if busy || idle {
                self.focus_pane(pane);
                return;
            }
        }
        let dir = launchpad::expand(&l.dir).filter(|d| d.is_dir());
        let start = dir.clone().or_else(|| self.focused_cwd());
        let here = self.tab().map(|t| t.focus).filter(|p| l.run_in == RunIn::Here && self.panes.get(p).is_some_and(|p| p.foreground().1));
        let (pane, fresh) = match (here, l.run_in) {
            (Some(p), _) => (Some(p), false),
            (None, RunIn::Split) => (self.split_at(Orientation::SideBySide, start), true),
            _ => (self.open_tab_at(start), true),
        };
        let Some(pane) = pane else { return };
        // A new shell already starts in the folder. Anywhere else, cd there first; a folder that's
        // gone then fails visibly in the pane instead of running the commands in the wrong place.
        let line = launchpad::command_line(&l, !(fresh && dir.is_some()));
        self.launches.insert(id, Run::new(pane, line));
    }

    /// Every frame: forget closed panes and deleted launchers, type queued command lines once their
    /// shell is ready, and refresh the statuses the panel, rail and tabs show.
    pub(super) fn launch_tick(&mut self, ctx: &egui::Context) {
        let panes = &self.panes;
        let launchers = &self.config.launchers;
        self.launches.retain(|id, r| panes.contains_key(&r.pane) && launchers.iter().any(|l| l.id == *id));
        if self.launches.is_empty() {
            self.launch_status.clear();
            return;
        }
        let now = Instant::now();
        let mut waiting = false;
        for run in self.launches.values_mut() {
            let Some((line, since)) = &run.queued else { continue };
            let p = &self.panes[&run.pane];
            let waited = now - *since;
            if p.foreground().1 && (waited > PROMPT_GRACE || p.at_empty_prompt()) {
                p.term.lock().scroll_display(Scroll::Bottom);
                p.write(format!("{line}\r").into_bytes());
                run.queued = None;
                run.sent = Some(now);
                run.active = true;
                run.fg.clear();
                run.idle_reads = 0;
            } else if waited > QUEUE_TIMEOUT {
                run.queued = None;
            }
            waiting = true;
        }
        if self.launch_checked.is_none_or(|t| now - t >= STATUS_EVERY) {
            self.launch_checked = Some(now);
            let mut status = launchpad::Statuses::new();
            for (id, run) in self.launches.iter_mut() {
                if run.active {
                    let (fg, idle) = self.panes[&run.pane].foreground();
                    if !idle {
                        run.fg = fg;
                        run.idle_reads = 0;
                    } else if run.sent.is_some_and(|t| now - t > Duration::from_millis(800)) {
                        // Between `a && b` the shell has the foreground for a moment, so one idle
                        // read doesn't mean the line is done.
                        run.idle_reads += 1;
                        run.active = run.idle_reads < 2;
                    }
                }
                let state = match (&run.queued, run.active) {
                    (Some(_), _) => State::Starting,
                    (None, true) if run.fg.is_empty() => State::Starting,
                    (None, true) => State::Running(run.fg.clone()),
                    (None, false) => State::Finished,
                };
                let tab = self.tabs.iter().position(|t| t.root.contains(run.pane)).map_or(0, |i| i + 1);
                status.insert(*id, Status { state, tab });
            }
            self.launch_status = status;
        }
        // Keep looking while something is about to change that wouldn't repaint by itself: a
        // program that prints nothing at first, or a line that seems done (one more idle read).
        let settling = self.launches.values().any(|r| r.active && (r.idle_reads > 0 || r.sent.is_some_and(|t| now - t < Duration::from_secs(1))));
        if waiting || settling {
            ctx.request_repaint_after(Duration::from_millis(if waiting { 100 } else { 300 }));
        }
    }

    /// The name of the launcher running in `pane`, for its tab's label.
    pub(super) fn launched(&self, pane: PaneId) -> Option<&str> {
        let (id, _) = self.launches.iter().find(|(_, r)| r.pane == pane)?;
        self.launch_status.get(id).filter(|s| s.busy())?;
        self.config.launchers.iter().find(|l| l.id == *id).map(|l| l.name.as_str())
    }

    /// Whether any launcher is starting or running (the rail shows a dot).
    pub(super) fn any_launch_busy(&self) -> bool {
        self.launch_status.values().any(Status::busy)
    }
}

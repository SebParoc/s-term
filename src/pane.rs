use std::cell::Cell;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::CursorShape;
use alacritty_terminal::tty;

pub type PaneId = u64;

/// Forwards terminal events (title changes, child exit, replies...) to the UI thread.
#[derive(Clone)]
pub struct Listener {
    id: PaneId,
    tx: Sender<(PaneId, Event)>,
    ctx: egui::Context,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.tx.send((self.id, event));
        self.ctx.request_repaint();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

fn window_size(size: GridSize, cell: egui::Vec2) -> WindowSize {
    WindowSize {
        num_lines: size.rows as u16,
        num_cols: size.cols as u16,
        cell_width: cell.x.round() as u16,
        cell_height: cell.y.round() as u16,
    }
}

/// One terminal session: a shell on a PTY plus its emulator state.
pub struct Pane {
    pub term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    pub title: String,
    pub size: GridSize,
    cell: egui::Vec2,
    child_pid: u32,
    /// Fractional wheel scroll not yet applied, in lines.
    pub scroll_accum: f32,
    pub cursor_anim: crate::render::CursorAnim,
    /// `ai_tool`'s last answer and when it was read.
    ai: Cell<(Option<std::time::Instant>, Option<&'static str>)>,
}

impl Pane {
    pub fn spawn(
        id: PaneId,
        tx: Sender<(PaneId, Event)>,
        ctx: egui::Context,
        cwd: Option<PathBuf>,
        size: GridSize,
        cell: egui::Vec2,
        shell: &str,
        scrollback: usize,
    ) -> anyhow::Result<Self> {
        let mut opts = tty::Options::default();
        opts.working_directory = cwd;
        let mut words = shell.split_whitespace().map(String::from);
        if let Some(program) = words.next() {
            opts.shell = Some(tty::Shell::new(program, words.collect()));
        }
        opts.env.insert("TERM".into(), "xterm-256color".into());
        opts.env.insert("COLORTERM".into(), "truecolor".into());
        opts.env.insert("TERM_PROGRAM".into(), "s-term".into());

        let pty = tty::new(&opts, window_size(size, cell), id)?;
        let child_pid = pty.child().id();

        let listener = Listener { id, tx, ctx };
        let term = Arc::new(FairMutex::new(Term::new(Config {
                scrolling_history: scrollback,
                // Shown in "Auto" cursor mode when the program doesn't ask for a shape.
                default_cursor_style: alacritty_terminal::vte::ansi::CursorStyle { shape: CursorShape::Beam, blinking: false },
                ..Config::default()
            }, &size, listener.clone())));

        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();

        Ok(Self {
            term,
            notifier,
            title: "shell".into(),
            size,
            cell,
            child_pid,
            scroll_accum: 0.0,
            cursor_anim: Default::default(),
            ai: Cell::new((None, None)),
        })
    }

    pub fn write(&self, bytes: Vec<u8>) {
        self.notifier.notify(bytes);
    }

    pub fn window_size(&self) -> WindowSize {
        window_size(self.size, self.cell)
    }

    pub fn resize(&mut self, size: GridSize, cell: egui::Vec2) {
        if size == self.size && cell == self.cell {
            return;
        }
        self.size = size;
        self.cell = cell;
        self.term.lock().resize(size);
        self.notifier.on_resize(window_size(size, cell));
    }

    /// Current working directory of the shell (used so new splits/tabs open in the same place).
    pub fn cwd(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.child_pid)).ok()
    }

    /// The program in the foreground of this pane: its name, and whether it is the shell itself
    /// (i.e. the pane is idle at a prompt).
    pub fn foreground(&self) -> (String, bool) {
        let comm = |pid: u32| std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|s| s.trim_end().to_string()).unwrap_or_default();
        match self.foreground_pgid() {
            Some(pg) => {
                let name = comm(pg);
                if name.is_empty() {
                    (comm(self.child_pid), true)
                } else {
                    (name, false)
                }
            }
            None => (comm(self.child_pid), true),
        }
    }

    /// The AI coding CLI in the foreground of this pane: `claude`, `codex` or `agy`. Tabs, pane headers
    /// and the status meter ask every frame, so /proc is read at most every 500 ms.
    pub fn ai_tool(&self) -> Option<&'static str> {
        let (at, tool) = self.ai.get();
        if at.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500)) {
            return tool;
        }
        let tool = self.foreground_pgid().and_then(ai_tool_of);
        self.ai.set((Some(std::time::Instant::now()), tool));
        tool
    }

    /// The process group in the foreground of the terminal when it isn't the shell's own: the
    /// program running in this pane (its group leader's pid).
    pub fn foreground_pgid(&self) -> Option<u32> {
        // /proc/<pid>/stat: "pid (comm) state ppid pgrp session tty_nr tpgid ..."; comm may contain spaces.
        let tpgid = std::fs::read_to_string(format!("/proc/{}/stat", self.child_pid))
            .ok()
            .and_then(|s| s.rsplit_once(')').map(|(_, rest)| rest.to_string()))
            .and_then(|rest| rest.split_whitespace().nth(5).and_then(|t| t.parse::<i64>().ok()))?;
        (tpgid > 0 && tpgid as u32 != self.child_pid).then_some(tpgid as u32)
    }

    /// The shell is in the foreground and nothing is typed at its prompt: the text before the
    /// cursor ends in a prompt character (❯ $ # % >) and nothing follows the cursor.
    /// A heuristic (there is no prompt marker to go by); when unsure it says no.
    pub fn at_empty_prompt(&self) -> bool {
        use alacritty_terminal::index::Column;
        if !self.foreground().1 {
            return false;
        }
        let term = self.term.lock();
        let grid = term.grid();
        let cur = grid.cursor.point;
        let row = &grid[cur.line];
        let cols = grid.columns();
        let text = |a: usize, b: usize| (a..b).map(|c| row[Column(c)].c).collect::<String>();
        let col = cur.column.0.min(cols);
        let before = text(0, col);
        let after = text(col, cols);
        after.trim().is_empty() && before.trim_end().ends_with(['❯', '$', '#', '%', '>'])
    }

    /// The last `max_lines` lines of the pane (scrollback included), as plain text with trailing
    /// blank lines removed.
    pub fn text_tail(&self, max_lines: usize) -> String {
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::Flags;
        let term = self.term.lock();
        let grid = term.grid();
        let history = grid.history_size() as i32;
        let screen = grid.screen_lines() as i32;
        let cols = grid.columns();
        let mut lines: Vec<String> = Vec::new();
        for l in -history..screen {
            let row = &grid[Line(l)];
            let mut s = String::with_capacity(cols);
            for c in 0..cols {
                let cell = &row[Column(c)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                s.push(cell.c);
                if let Some(zw) = cell.zerowidth() {
                    s.extend(zw);
                }
            }
            let wrapped = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            let s = s.trim_end().to_string();
            match lines.last_mut() {
                Some(prev) if prev.ends_with('\u{0}') => {
                    prev.pop();
                    prev.push_str(&s);
                }
                _ => lines.push(s),
            }
            if wrapped {
                if let Some(last) = lines.last_mut() {
                    last.push('\u{0}');
                }
            }
        }
        for l in &mut lines {
            if l.ends_with('\u{0}') {
                l.pop();
            }
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].join("\n")
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        // Stops the IO thread; dropping the PTY there hangs up the shell.
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

/// Which AI coding CLI process `pid` is: by its name, or for a script runner (an npm install runs
/// `node …/@anthropic-ai/claude-code/cli.js` or `node …/@openai/codex/bin/codex.js`) by its script.
fn ai_tool_of(pid: u32) -> Option<&'static str> {
    const TOOLS: [&str; 3] = ["claude", "codex", "agy"];
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim_end();
    if let Some(t) = TOOLS.iter().find(|t| **t == comm) {
        return Some(t);
    }
    if !matches!(comm, "node" | "bun" | "deno") {
        return None;
    }
    let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    cmd.split(|b| *b == 0).skip(1).take(3).find_map(|a| {
        let a = String::from_utf8_lossy(a).to_ascii_lowercase();
        if a.contains("claude-code") || a.ends_with("/claude") {
            Some("claude")
        } else if a.contains("@openai/codex") || a.ends_with("/codex") || a.ends_with("/codex.js") {
            Some("codex")
        } else {
            None
        }
    })
}

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
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
        let term = Arc::new(FairMutex::new(Term::new(Config { scrolling_history: scrollback, ..Config::default() }, &size, listener.clone())));

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

}

impl Drop for Pane {
    fn drop(&mut self) {
        // Stops the IO thread; dropping the PTY there hangs up the shell.
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

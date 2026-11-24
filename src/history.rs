//! History: closed tabs and panes, and the windows' last sessions, kept so they can be reopened.
//!
//! A closed pane can't come back to life (closing it hangs up its shell), so an entry keeps what it
//! takes to start it again: the split layout, and for each pane its folder, title and the program
//! that was running. For Claude Code and Codex that includes the conversation id, so reopening
//! resumes the conversation. Any other program is typed at the new prompt but not run.
//!
//! Files, in `~/.local/share/s-term/` and shared by every S-term window:
//! - `history.json`: the entries, newest first. Each change re-reads the file first, so windows
//!   don't drop each other's entries.
//! - `sessions/<pid>.json`: each open window's tabs, rewritten as they change. At startup the files
//!   of windows that are gone (a crash, a logout) become session entries, like a normal quit.
//!
//! This file holds the data, the files and the panel. The app (`app/history.rs`) records panes as
//! they close, keeps the session file, and restores entries.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Rect, RichText, Sense};
use serde::{Deserialize, Serialize};

use crate::app::CLICK;
use crate::icons::{self, Icon};
use crate::layout::{Node, Orientation};
use crate::pane::PaneId;
use crate::sidebar::UiAction;
use crate::theme::{c32, lerp_color, Theme};

/// Entries kept; the oldest go first.
const MAX_ENTRIES: usize = 60;
/// How often the file is checked for entries other windows added.
const RELOAD_EVERY: Duration = Duration::from_secs(2);
/// A change of titles alone (Claude Code animates its own) is written at most this often.
const TITLES_EVERY: Duration = Duration::from_secs(30);
/// Longer command lines aren't typed again.
const MAX_COMMAND: usize = 400;
const ROW_H: f32 = 46.0;

// ------------------------------------------------------------------------------------------- data

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Claude,
    Codex,
}

impl AgentKind {
    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
        }
    }
}

/// An AI agent's conversation, resumed on restore.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub struct Agent {
    pub kind: AgentKind,
    /// The conversation id (`claude --resume <id>`, `codex resume <id>`).
    pub session: String,
}

/// One pane, as it was when it closed.
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug, Default)]
#[serde(default)]
pub struct PaneRec {
    pub cwd: Option<PathBuf>,
    /// The title the shell or the program gave it.
    pub title: String,
    /// The foreground program's command line; empty when the shell was at its prompt.
    pub command: Vec<String>,
    pub agent: Option<Agent>,
}

/// A tab's split tree.
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    Pane(PaneRec),
    Split { side_by_side: bool, ratio: f32, a: Box<Layout>, b: Box<Layout> },
}

impl Layout {
    pub fn panes(&self) -> Vec<&PaneRec> {
        fn walk<'a>(l: &'a Layout, out: &mut Vec<&'a PaneRec>) {
            match l {
                Layout::Pane(p) => out.push(p),
                Layout::Split { a, b, .. } => {
                    walk(a, out);
                    walk(b, out);
                }
            }
        }
        let mut v = Vec::new();
        walk(self, &mut v);
        v
    }

    fn each_mut(&mut self, f: &mut impl FnMut(&mut PaneRec)) {
        match self {
            Layout::Pane(p) => f(p),
            Layout::Split { a, b, .. } => {
                a.each_mut(f);
                b.each_mut(f);
            }
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub struct TabRec {
    pub layout: Layout,
    /// The focused pane, counted in layout order.
    #[serde(default)]
    pub focus: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A whole tab (or the last pane of one).
    #[default]
    Tab,
    /// One pane of a split tab.
    Pane,
    /// All the tabs of a window that closed.
    Session,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug, Default)]
#[serde(default)]
pub struct Entry {
    pub id: u64,
    pub kind: Kind,
    /// When it closed (unix seconds).
    pub closed: i64,
    pub tabs: Vec<TabRec>,
    /// The tab that was active (sessions).
    pub active: usize,
    /// The startup popup offered this session already.
    pub offered: bool,
}

impl Entry {
    pub fn panes(&self) -> Vec<&PaneRec> {
        self.tabs.iter().flat_map(|t| t.layout.panes()).collect()
    }

    /// A lone shell at its prompt in the home folder isn't worth keeping.
    fn trivial(&self) -> bool {
        match self.panes().as_slice() {
            [] => true,
            [p] => p.command.is_empty() && p.agent.is_none() && p.cwd.as_deref().map_or(true, |d| d.parent().is_none() || home().as_deref() == Some(d)),
            _ => false,
        }
    }

    /// What it holds, titles aside. Of two entries alike, only the newer is kept.
    fn signature(&self) -> String {
        let mut tabs = self.tabs.clone();
        for t in &mut tabs {
            t.layout.each_mut(&mut |p| p.title.clear());
        }
        serde_json::to_string(&tabs).unwrap_or_default()
    }

    /// Its panes' folder names, each once, in order.
    pub fn folders(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for p in self.panes() {
            let n = folder_name(p.cwd.as_deref());
            if !v.contains(&n) {
                v.push(n);
            }
        }
        v
    }

    /// The panel's two lines: what it is, and what was in it.
    pub fn describe(&self) -> (String, String) {
        let folders = self.folders();
        let list = |n: usize, sep: &str| {
            let mut s = folders.iter().take(n).cloned().collect::<Vec<_>>().join(sep);
            if folders.len() > n {
                s += &format!(" +{}", folders.len() - n);
            }
            s
        };
        let panes = self.panes();
        match (self.kind, panes.as_slice()) {
            (Kind::Session, _) => (format!("Session · {}", plural(self.tabs.len(), "tab")), list(4, ", ")),
            (_, [p]) => (folder_name(p.cwd.as_deref()), pane_detail(p)),
            (_, ps) => {
                let mut progs: Vec<String> = Vec::new();
                for p in ps {
                    if let Some(n) = program(p).filter(|n| !progs.contains(n)) {
                        progs.push(n);
                    }
                }
                let what = if progs.is_empty() { String::new() } else { format!(" · {}", progs.join(", ")) };
                (list(3, " + "), format!("{}{what}", plural(ps.len(), "pane")))
            }
        }
    }
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// `s-term` for `~/s-term`, `~` for the home folder.
fn folder_name(cwd: Option<&Path>) -> String {
    match cwd {
        None => "terminal".into(),
        Some(d) if home().as_deref() == Some(d) => "~".into(),
        Some(d) => d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| d.display().to_string()),
    }
}

/// A title worth showing: without the spinner or status mark in front (Claude Code's `✳`), and
/// not just the shell's `user@host:folder`.
pub fn clean_title(t: &str) -> Option<String> {
    let t = t.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
    let plain = t.is_empty() || t == "shell" || (t.contains('@') && t.contains(':'));
    (!plain).then(|| t.to_string())
}

/// The short name of what a pane was running, for lists.
fn program(p: &PaneRec) -> Option<String> {
    if let Some(a) = &p.agent {
        return Some(if a.kind == AgentKind::Claude { "claude".into() } else { "codex".into() });
    }
    let first = p.command.first()?;
    Some(Path::new(first).file_name().map_or_else(|| first.clone(), |n| n.to_string_lossy().into_owned()))
}

/// A single pane's second line: the agent and its topic, the command, or the folder.
fn pane_detail(p: &PaneRec) -> String {
    let title = clean_title(&p.title);
    match (&p.agent, p.command.is_empty()) {
        (Some(a), _) => match title {
            Some(t) => format!("{} · {t}", a.kind.name()),
            None => format!("{} conversation", a.kind.name()),
        },
        (None, false) => command_line(&p.command),
        (None, true) => p.cwd.as_deref().map(crate::launchpad::tilde).unwrap_or_default(),
    }
}

// ------------------------------------------------------------------------------- split tree

/// The layout of a split tree, with `rec` describing each pane.
pub fn layout_of(node: &Node, rec: &mut impl FnMut(PaneId) -> PaneRec) -> Layout {
    match node {
        Node::Leaf(id) => Layout::Pane(rec(*id)),
        Node::Split { orient, ratio, a, b } => {
            Layout::Split { side_by_side: *orient == Orientation::SideBySide, ratio: *ratio, a: Box::new(layout_of(a, rec)), b: Box::new(layout_of(b, rec)) }
        }
    }
}

/// A split tree shaped like `layout`, with a pane from `spawn` for each of its panes. A pane that
/// doesn't start leaves its sibling in its place; None when none started.
pub fn build(layout: &Layout, spawn: &mut impl FnMut(&PaneRec) -> Option<PaneId>) -> Option<Node> {
    match layout {
        Layout::Pane(p) => spawn(p).map(Node::Leaf),
        Layout::Split { side_by_side, ratio, a, b } => match (build(a, spawn), build(b, spawn)) {
            (Some(a), Some(b)) => {
                let orient = if *side_by_side { Orientation::SideBySide } else { Orientation::Stacked };
                Some(Node::Split { orient, ratio: ratio.clamp(0.05, 0.95), a: Box::new(a), b: Box::new(b) })
            }
            (x, None) | (None, x) => x,
        },
    }
}

// ---------------------------------------------------------------------------- what's running

/// What a pane's foreground process group `pgid` runs: its leader's command line, and the agent
/// conversation in it, if there is one.
pub fn probe(pgid: u32) -> (Vec<String>, Option<Agent>) {
    let argv = cmdline(pgid);
    // `claude`, or a wrapper running it (`node …/claude-code/cli.js`, `npx codex`).
    let head = argv.iter().take(2).map(|a| a.to_ascii_lowercase()).collect::<Vec<_>>().join(" ");
    let agent = if head.contains("claude") {
        in_group(pgid, claude_session).map(|session| Agent { kind: AgentKind::Claude, session })
    } else if head.contains("codex") {
        in_group(pgid, codex_session).map(|session| Agent { kind: AgentKind::Codex, session })
    } else {
        None
    };
    (argv, agent)
}

/// A process's command line, split into arguments.
fn cmdline(pid: u32) -> Vec<String> {
    let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { return Vec::new() };
    let argv: Vec<String> = raw.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    retitled(argv)
}

/// A program that renamed itself (`npm` shows `npm run dev` as a single argument) is split back.
fn retitled(argv: Vec<String>) -> Vec<String> {
    match argv.as_slice() {
        [one] if one.contains(' ') => one.split_whitespace().map(String::from).collect(),
        _ => argv,
    }
}

/// `find` on the group's leader, else on the other processes in the group.
fn in_group(pgid: u32, find: fn(u32) -> Option<String>) -> Option<String> {
    find(pgid).or_else(|| group(pgid).into_iter().find_map(find))
}

/// The other processes in process group `pgid`.
fn group(pgid: u32) -> Vec<u32> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return v };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        // /proc/<pid>/stat: "pid (comm) state ppid pgrp ..."; comm may contain spaces.
        let pg = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| s.rsplit_once(')')?.1.split_whitespace().nth(2)?.parse::<u32>().ok());
        if pid != pgid && pg == Some(pgid) {
            v.push(pid);
        }
    }
    v
}

fn claude_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).or_else(|| home().map(|h| h.join(".claude")))
}

/// The conversation of the Claude Code process `pid`: Claude Code keeps `sessions/<pid>.json` up
/// to date while it runs (read only, never written).
fn claude_session(pid: u32) -> Option<String> {
    let raw = std::fs::read(claude_dir()?.join("sessions").join(format!("{pid}.json"))).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    v["sessionId"].as_str().filter(|s| valid_id(s)).map(String::from)
}

/// The conversation of the Codex process `pid`: the `rollout-…-<uuid>.jsonl` it has open.
fn codex_session(pid: u32) -> Option<String> {
    std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?.flatten().find_map(|e| rollout_id(&std::fs::read_link(e.path()).ok()?))
}

fn rollout_id(p: &Path) -> Option<String> {
    let name = p.file_name()?.to_str()?;
    let stem = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    (valid_id(id) && id.matches('-').count() == 4).then(|| id.to_string())
}

/// Conversation ids are typed into a shell, so only plain uuid-like ones are used.
fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

pub fn command_line(argv: &[String]) -> String {
    argv.iter().map(|a| crate::sidebar::quote(a)).collect::<Vec<_>>().join(" ")
}

/// The command that resumes `agent`'s conversation, keeping the flags it ran with that still
/// apply (model, permission mode). A prompt given on the command line isn't sent again.
pub fn resume_line(agent: &Agent, argv: &[String]) -> Option<String> {
    if !valid_id(&agent.session) {
        return None;
    }
    let mut out: Vec<String> = match agent.kind {
        AgentKind::Claude => vec!["claude".into(), "--resume".into(), agent.session.clone()],
        AgentKind::Codex => return Some(format!("codex resume {}", agent.session)),
    };
    let mut args = argv.iter().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dangerously-skip-permissions" => out.push(a.clone()),
            "--model" | "--permission-mode" => {
                if let Some(v) = args.next() {
                    out.extend([a.clone(), v.clone()]);
                }
            }
            s if s.starts_with("--model=") || s.starts_with("--permission-mode=") => out.push(a.clone()),
            _ => {}
        }
    }
    Some(command_line(&out))
}

/// What to type into a restored pane, and whether to run it. An agent's conversation resumes;
/// any other program is only typed at the prompt, so it doesn't run again by surprise.
pub fn restore_line(p: &PaneRec) -> Option<(String, bool)> {
    let (line, run) = match p.agent.as_ref().and_then(|a| resume_line(a, &p.command)) {
        Some(l) => (l, true),
        None => (command_line(&p.command), false),
    };
    // A control character (a newline) would run the line on its own.
    (!line.is_empty() && line.len() <= MAX_COMMAND && !line.chars().any(char::is_control)).then_some((line, run))
}

// ------------------------------------------------------------------------------------------ files

#[derive(Deserialize, Default)]
struct HistoryFile {
    #[serde(default)]
    entries: Vec<Entry>,
}

#[derive(Serialize)]
struct HistoryOut<'a> {
    entries: &'a [Entry],
}

#[derive(Serialize, Deserialize)]
struct SessionFile {
    pid: u32,
    updated: i64,
    #[serde(default)]
    active: usize,
    #[serde(default)]
    tabs: Vec<TabRec>,
}

/// `~/.local/share/s-term`.
pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).or_else(|| home().map(|h| h.join(".local/share")))?;
    Some(base.join("s-term"))
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Writes the file whole (a temp file, then a rename), readable by the user only: it holds folder
/// names and command lines.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// Whether `pid` is a running S-term.
fn s_term_alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim_end() == "s-term")
}

fn new_id(v: &[Entry]) -> u64 {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |d| d.as_millis() as u64);
    ms.max(v.iter().map(|e| e.id).max().unwrap_or(0) + 1)
}

/// The entries, and this window's session file.
#[derive(Default)]
pub struct History {
    /// Newest first.
    pub entries: Vec<Entry>,
    /// Where the files are. None keeps everything in memory (screenshots, tests).
    dir: Option<PathBuf>,
    /// `history.json`'s modification time when last read or written.
    seen: Option<SystemTime>,
    checked: Option<Instant>,
    /// The session as last written: in full, without titles, and when.
    session_full: String,
    session_key: String,
    session_at: Option<Instant>,
}

impl History {
    /// The history kept in `dir` (None: in memory only).
    pub fn open(dir: Option<PathBuf>) -> Self {
        let mut h = Self { dir, ..Default::default() };
        h.reload();
        h
    }

    fn file(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join("history.json"))
    }

    fn sessions_dir(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join("sessions"))
    }

    fn reload(&mut self) {
        let Some(p) = self.file() else { return };
        self.seen = mtime(&p);
        self.entries = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<HistoryFile>(&b).ok()).map(|f| f.entries).unwrap_or_default();
    }

    /// Makes a change on top of what's on disk (another window may have written since) and saves it.
    fn change(&mut self, f: impl FnOnce(&mut Vec<Entry>)) {
        self.reload();
        f(&mut self.entries);
        // Stable: a new entry stays above older ones from the same second.
        self.entries.sort_by(|a, b| b.closed.cmp(&a.closed));
        self.entries.truncate(MAX_ENTRIES);
        let Some(p) = self.file() else { return };
        let json = serde_json::to_vec_pretty(&HistoryOut { entries: &self.entries }).unwrap_or_default();
        if let Err(e) = write_private(&p, &json) {
            eprintln!("s-term: history: {}: {e}", p.display());
        }
        self.seen = mtime(&p);
    }

    /// Picks up entries other windows added (checks the file every couple of seconds).
    pub fn refresh(&mut self) {
        if self.checked.is_some_and(|t| t.elapsed() < RELOAD_EVERY) {
            return;
        }
        self.checked = Some(Instant::now());
        if self.file().is_some_and(|p| mtime(&p) != self.seen) {
            self.reload();
        }
    }

    /// Adds an entry on top (and drops an older one just like it). A lone idle shell at home isn't kept.
    pub fn push(&mut self, mut e: Entry) {
        if e.trivial() {
            return;
        }
        let sig = e.signature();
        self.change(|v| {
            e.id = new_id(v);
            v.retain(|x| x.signature() != sig);
            v.insert(0, e);
        });
    }

    /// Removes an entry and hands it over (to be restored).
    pub fn take(&mut self, id: u64) -> Option<Entry> {
        let mut out = None;
        self.change(|v| {
            if let Some(i) = v.iter().position(|e| e.id == id) {
                out = Some(v.remove(i));
            }
        });
        out
    }

    pub fn clear(&mut self) {
        self.change(Vec::clear);
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The session to offer at startup: the newest entry, if it's a window's session not offered before.
    pub fn offer(&self) -> Option<&Entry> {
        self.entries.first().filter(|e| e.kind == Kind::Session && !e.offered)
    }

    pub fn mark_offered(&mut self, id: u64) {
        self.change(|v| v.iter_mut().filter(|e| e.id == id).for_each(|e| e.offered = true));
    }

    /// The session files of windows that are gone (a crash, a logout) become session entries.
    pub fn adopt_orphans(&mut self) {
        let Some(dir) = self.sessions_dir() else { return };
        let Ok(rd) = std::fs::read_dir(&dir) else { return };
        let me = std::process::id();
        let mut found = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Some(pid) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) else { continue };
            if pid == me || s_term_alive(pid) {
                continue;
            }
            if let Some(s) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<SessionFile>(&b).ok()) {
                found.push(Entry { kind: Kind::Session, closed: s.updated, tabs: s.tabs, active: s.active, ..Default::default() });
            }
            let _ = std::fs::remove_file(&p);
        }
        for e in found {
            self.push(e);
        }
    }

    /// Whether another S-term window is open.
    pub fn others_running(&self) -> bool {
        let me = std::process::id();
        let Some(Ok(rd)) = self.sessions_dir().map(std::fs::read_dir) else { return false };
        rd.flatten().filter_map(|e| e.path().file_stem()?.to_str()?.parse::<u32>().ok()).any(|pid| pid != me && s_term_alive(pid))
    }

    /// Keeps this window's session file up to date. Cheap to call often: it writes only on change.
    pub fn write_session(&mut self, tabs: &[TabRec], active: usize) {
        let pid = std::process::id();
        let Some(p) = self.sessions_dir().map(|d| d.join(format!("{pid}.json"))) else { return };
        let full = serde_json::to_string(&(tabs, active)).unwrap_or_default();
        if full == self.session_full {
            return;
        }
        let mut untitled = tabs.to_vec();
        for t in &mut untitled {
            t.layout.each_mut(&mut |p| p.title.clear());
        }
        let key = serde_json::to_string(&(&untitled, active)).unwrap_or_default();
        if key == self.session_key && self.session_at.is_some_and(|t| t.elapsed() < TITLES_EVERY) {
            return;
        }
        let json = serde_json::to_vec(&SessionFile { pid, updated: now(), active, tabs: tabs.to_vec() }).unwrap_or_default();
        if let Err(e) = write_private(&p, &json) {
            eprintln!("s-term: history: {}: {e}", p.display());
        }
        (self.session_full, self.session_key, self.session_at) = (full, key, Some(Instant::now()));
    }

    /// The window is closing: its session file goes, and its open tabs become a session entry.
    pub fn end_session(&mut self, tabs: Vec<TabRec>, active: usize) {
        if let Some(p) = self.sessions_dir().map(|d| d.join(format!("{}.json", std::process::id()))) {
            let _ = std::fs::remove_file(p);
        }
        self.push(Entry { kind: Kind::Session, closed: now(), tabs, active, ..Default::default() });
    }
}

// ------------------------------------------------------------------------------------------ panel

/// What the panel asks the app to do with an entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    /// Reopen it as new tabs.
    Restore,
    /// Reopen a single pane as a split of the focused one.
    Split,
    /// Open the Launchpad's editor on a launcher for its folder.
    Launcher,
    Delete,
    /// Delete every entry (the id doesn't matter).
    Clear,
}

#[derive(Default)]
pub struct HistoryUi {
    /// "Clear history" was clicked and waits to be confirmed.
    pub confirm_clear: bool,
}

/// The History view.
pub fn panel(ui: &mut egui::Ui, theme: &Theme, st: &mut HistoryUi, hist: &History, actions: &mut Vec<UiAction>) {
    st.confirm_clear &= !hist.entries.is_empty();
    if st.confirm_clear {
        confirm_clear(ui, theme, st, hist.entries.len(), actions);
    }
    if hist.entries.is_empty() {
        empty_state(ui, theme);
        return;
    }
    let today = crate::usage::day_of(now());
    let mut group = None;
    for e in &hist.entries {
        let g = match today - crate::usage::day_of(e.closed) {
            ..=0 => "Today",
            1 => "Yesterday",
            2..=6 => "This week",
            _ => "Older",
        };
        if group != Some(g) {
            ui.add_space(if group.is_some() { 8.0 } else { 0.0 });
            ui.label(RichText::new(g).size(11.0).color(theme.muted));
            ui.add_space(2.0);
            group = Some(g);
        }
        row(ui, theme, e, actions);
        ui.add_space(4.0);
    }
    ui.add_space(4.0);
    ui.label(RichText::new("Click to reopen. Right-click for more. Ctrl+Shift+R reopens the newest.").size(10.5).color(theme.muted));
}

fn confirm_clear(ui: &mut egui::Ui, theme: &Theme, st: &mut HistoryUi, n: usize, actions: &mut Vec<UiAction>) {
    crate::usage_ui::card(ui, theme, |ui| {
        ui.label(RichText::new("Clear the history?").size(13.0).strong().color(theme.text));
        let what = if n == 1 { "1 entry".to_string() } else { format!("{n} entries") };
        ui.label(RichText::new(format!("Forgets {what}. Open tabs aren't affected.")).size(11.5).color(theme.muted));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let red = c32(theme.palette[1]);
            if ui.add(egui::Button::new(RichText::new("Clear").color(Color32::WHITE)).fill(red).sense(CLICK)).clicked() {
                actions.push(UiAction::History(0, Op::Clear));
                st.confirm_clear = false;
            }
            if ui.add(egui::Button::new("Cancel").sense(CLICK)).clicked() {
                st.confirm_clear = false;
            }
        });
    });
}

fn empty_state(ui: &mut egui::Ui, theme: &Theme) {
    crate::usage_ui::card(ui, theme, |ui| {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 17.0, theme.accent.gamma_multiply(0.16));
            icons::draw(ui.painter(), Icon::History, icons::center_rect(r.center(), 16.0), theme.accent);
            ui.vertical(|ui| {
                ui.label(RichText::new("Nothing closed yet").size(13.5).strong().color(theme.text));
                ui.label(RichText::new("Closed tabs and panes land here").size(11.5).color(theme.muted));
            });
        });
        ui.add_space(8.0);
        ui.label(
            RichText::new("Reopen one with a click: its folders and splits come back, and Claude Code or Codex conversations resume where they were. Ctrl+Shift+R reopens the last one.")
                .size(11.5)
                .color(theme.muted),
        );
    });
}

fn entry_icon(e: &Entry, theme: &Theme) -> (Icon, Color32) {
    let panes = e.panes();
    if e.kind == Kind::Session {
        (Icon::Restore, theme.accent)
    } else if panes.len() > 1 {
        (Icon::SplitRight, c32(theme.palette[4]))
    } else if panes.iter().any(|p| p.agent.is_some()) {
        (Icon::Sparkle, theme.accent2)
    } else if panes.iter().any(|p| !p.command.is_empty()) {
        (Icon::Terminal, c32(theme.palette[2]))
    } else {
        (Icon::Folder, theme.muted)
    }
}

fn ago(closed: i64) -> String {
    match (now() - closed).max(0) {
        0..=59 => "now".into(),
        s @ 60..=3599 => format!("{}m", s / 60),
        s @ 3600..=86399 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

/// One entry: an icon for its kind, what it is and what was in it, and when it closed (a delete
/// button on hover).
fn row(ui: &mut egui::Ui, theme: &Theme, e: &Entry, actions: &mut Vec<UiAction>) {
    let id = Id::new(("s-term-hist", e.id));
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(r, id, CLICK);
    // The delete button sits on top of the row, so "hovered" would go off over it.
    let hover = ui.ctx().animate_bool_with_time(id.with("h"), ui.rect_contains_pointer(r), 0.1);
    let p = ui.painter();
    p.rect_filled(r, 10.0, lerp_color(theme.header.gamma_multiply(0.45), theme.header, hover));

    let (icon, tint) = entry_icon(e, theme);
    let ic = pos2(r.min.x + 22.0, r.center().y);
    p.circle_filled(ic, 14.0, tint.gamma_multiply(0.16));
    icons::draw(p, icon, icons::center_rect(ic, 14.0), tint);

    let xr = Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(24.0, 24.0));
    let del = ui.interact(xr, id.with("del"), CLICK);
    if hover > 0.0 {
        if del.hovered() {
            p.rect_filled(xr, 7.0, theme.header_focused);
        }
        let col = if del.hovered() { c32(theme.palette[1]) } else { theme.muted };
        icons::draw(p, Icon::Close, icons::center_rect(xr.center(), 11.0), col.gamma_multiply(hover));
    }
    if hover < 1.0 {
        p.text(pos2(r.max.x - 10.0, r.center().y), Align2::RIGHT_CENTER, ago(e.closed), FontId::proportional(10.5), theme.muted.gamma_multiply(1.0 - hover));
    }

    let (head, detail) = e.describe();
    let x = r.min.x + 44.0;
    let clip = p.with_clip_rect(Rect::from_min_max(r.min, pos2(r.max.x - 36.0, r.max.y)));
    clip.text(pos2(x, r.min.y + 15.5), Align2::LEFT_CENTER, head, FontId::proportional(13.0), theme.text);
    clip.text(pos2(x, r.min.y + 31.5), Align2::LEFT_CENTER, detail, FontId::monospace(10.5), theme.muted);

    if del.on_hover_text("Delete").clicked() {
        actions.push(UiAction::History(e.id, Op::Delete));
    }
    let resp = resp.on_hover_ui(|ui| {
        for (i, t) in e.tabs.iter().enumerate() {
            if e.tabs.len() > 1 {
                ui.label(RichText::new(format!("Tab {}", i + 1)).strong());
            }
            for p in t.layout.panes() {
                let dir = p.cwd.as_deref().map(crate::launchpad::tilde).unwrap_or_else(|| "?".into());
                ui.label(RichText::new(dir).monospace());
                let what = match (&p.agent, p.command.is_empty()) {
                    (Some(a), _) => format!("{} · resumes the conversation", a.kind.name()),
                    (None, false) => format!("{} · typed at the prompt, not run", command_line(&p.command)),
                    (None, true) => String::new(),
                };
                if !what.is_empty() {
                    ui.label(RichText::new(format!("  {what}")).color(theme.muted));
                }
            }
        }
    });
    if resp.clicked() {
        actions.push(UiAction::History(e.id, Op::Restore));
    }
    let panes = e.panes();
    let dir = panes.first().and_then(|p| p.cwd.clone());
    resp.context_menu(|ui| {
        let item = |ui: &mut egui::Ui, label: &str| {
            let hit = ui.add(egui::Button::new(label)).clicked();
            if hit {
                ui.close_menu();
            }
            hit
        };
        if item(ui, if e.kind == Kind::Session { "Reopen its tabs" } else { "Reopen in a new tab" }) {
            actions.push(UiAction::History(e.id, Op::Restore));
        }
        if panes.len() == 1 && item(ui, "Reopen as a split") {
            actions.push(UiAction::History(e.id, Op::Split));
        }
        if let Some(d) = &dir {
            if item(ui, "Copy folder path") {
                actions.push(UiAction::Copy(d.to_string_lossy().into_owned()));
            }
            if e.folders().len() == 1 && item(ui, "New launcher for this folder…") {
                actions.push(UiAction::History(e.id, Op::Launcher));
            }
        }
        ui.separator();
        if item(ui, "Delete") {
            actions.push(UiAction::History(e.id, Op::Delete));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(cwd: &str, command: &[&str]) -> PaneRec {
        PaneRec { cwd: Some(PathBuf::from(cwd)), title: String::new(), command: command.iter().map(|s| s.to_string()).collect(), agent: None }
    }

    fn entry(kind: Kind, closed: i64, panes: Vec<PaneRec>) -> Entry {
        let mut it = panes.into_iter();
        let mut layout = Layout::Pane(it.next().unwrap());
        for p in it {
            layout = Layout::Split { side_by_side: true, ratio: 0.5, a: Box::new(layout), b: Box::new(Layout::Pane(p)) };
        }
        Entry { kind, closed, tabs: vec![TabRec { layout, focus: 0 }], ..Default::default() }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("s-term-hist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn layout_round_trips_through_a_split_tree() {
        let mut root = Node::Leaf(1);
        root.split(1, 2, Orientation::SideBySide);
        root.split(2, 3, Orientation::Stacked);
        let l = layout_of(&root, &mut |id| pane(&format!("/srv/{id}"), &[]));
        let back: Layout = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
        let dirs: Vec<_> = l.panes().iter().map(|p| p.cwd.clone().unwrap()).collect();
        assert_eq!(dirs, [PathBuf::from("/srv/1"), PathBuf::from("/srv/2"), PathBuf::from("/srv/3")]);

        // Pane "2" doesn't start: "3" takes its place next to "1".
        let mut next = 10;
        let node = build(&l, &mut |p| {
            next += 1;
            (p.cwd.as_deref() != Some(Path::new("/srv/2"))).then_some(next)
        })
        .unwrap();
        let mut ids = Vec::new();
        node.leaves(&mut ids);
        assert_eq!(ids, [11, 13]);
        assert!(matches!(node, Node::Split { orient: Orientation::SideBySide, .. }));
        assert!(build(&l, &mut |_| None).is_none());
    }

    #[test]
    fn old_and_partial_files_load() {
        let f: HistoryFile = serde_json::from_str(r#"{"entries":[{"id":3,"tabs":[{"layout":{"pane":{"cwd":"/srv"}}}]}]}"#).unwrap();
        let e = &f.entries[0];
        assert_eq!((e.id, e.kind, e.offered), (3, Kind::Tab, false));
        assert_eq!(e.panes()[0].cwd.as_deref(), Some(Path::new("/srv")));
        assert!(serde_json::from_str::<HistoryFile>("{}").unwrap().entries.is_empty());
    }

    #[test]
    fn idle_shell_at_home_is_not_kept() {
        let Some(h) = home() else { return };
        let at_home = entry(Kind::Tab, 1, vec![pane(&h.to_string_lossy(), &[])]);
        assert!(at_home.trivial());
        assert!(entry(Kind::Tab, 1, vec![pane("/", &[])]).trivial());
        assert!(!entry(Kind::Tab, 1, vec![pane(&h.to_string_lossy(), &["htop"])]).trivial());
        assert!(!entry(Kind::Tab, 1, vec![pane("/srv/web", &[])]).trivial());
        assert!(!entry(Kind::Tab, 1, vec![pane(&h.to_string_lossy(), &[]), pane(&h.to_string_lossy(), &[])]).trivial());
    }

    #[test]
    fn push_dedups_caps_and_survives_other_windows() {
        let dir = temp_dir("push");
        let mut a = History::open(Some(dir.clone()));
        a.push(entry(Kind::Tab, 100, vec![pane("/srv/web", &[])]));
        a.push(entry(Kind::Tab, 200, vec![pane("/srv/api", &["cargo", "run"])]));
        // Another window adds one; this window's next change keeps it.
        let mut b = History::open(Some(dir.clone()));
        b.push(entry(Kind::Pane, 300, vec![pane("/srv/docs", &[])]));
        let mut same = entry(Kind::Tab, 400, vec![pane("/srv/web", &[])]);
        same.tabs[0].layout.each_mut(&mut |p| p.title = "a new title".into());
        a.push(same);
        let dirs: Vec<_> = a.entries.iter().map(|e| e.folders().join(",")).collect();
        assert_eq!(dirs, ["web", "docs", "api"]);
        assert_eq!(a.entries[0].closed, 400);

        let id = a.entries[1].id;
        assert_eq!(a.take(id).map(|e| e.kind), Some(Kind::Pane));
        assert!(a.take(id).is_none());
        b.refresh();
        assert_eq!(b.entries.len(), 2);

        for i in 0..(MAX_ENTRIES as i64 + 5) {
            a.push(entry(Kind::Tab, 1000 + i, vec![pane(&format!("/srv/p{i}"), &[])]));
        }
        assert_eq!(a.entries.len(), MAX_ENTRIES);
        assert_eq!(a.entries[0].closed, 1000 + MAX_ENTRIES as i64 + 4);
        a.clear();
        assert!(History::open(Some(dir.clone())).entries.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sessions_of_closed_windows_are_offered_once() {
        let dir = temp_dir("orphans");
        let mut h = History::open(Some(dir.clone()));
        h.push(entry(Kind::Tab, 50, vec![pane("/srv/old", &[])]));
        assert!(h.offer().is_none());
        // A window that's gone (no such pid) left its session file behind.
        let tabs = vec![TabRec { layout: Layout::Pane(pane("/srv/web", &[])), focus: 0 }, TabRec { layout: Layout::Pane(pane("/srv/api", &[])), focus: 0 }];
        let orphan = SessionFile { pid: 999_999_999, updated: 60, active: 1, tabs };
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::fs::write(dir.join("sessions/999999999.json"), serde_json::to_vec(&orphan).unwrap()).unwrap();
        assert!(!h.others_running());
        h.adopt_orphans();
        assert!(!dir.join("sessions/999999999.json").exists());
        let e = h.offer().expect("offered").clone();
        assert_eq!((e.kind, e.active, e.tabs.len(), e.closed), (Kind::Session, 1, 2, 60));
        assert_eq!(e.describe(), ("Session · 2 tabs".into(), "web, api".into()));
        h.mark_offered(e.id);
        assert!(History::open(Some(dir.clone())).offer().is_none());

        // This window's own session file comes and goes with it.
        let mine = dir.join(format!("sessions/{}.json", std::process::id()));
        h.write_session(&e.tabs, 0);
        assert!(mine.exists());
        h.end_session(e.tabs.clone(), 0);
        assert!(!mine.exists());
        assert_eq!(h.offer().map(|x| x.tabs.len()), Some(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_keeps_safe_flags_only() {
        let claude = Agent { kind: AgentKind::Claude, session: "08dbf0dd-5f37-4bb8-b4ed-dc8ec1731037".into() };
        let argv: Vec<String> = ["claude", "--model", "opus", "--dangerously-skip-permissions", "fix the login bug", "--permission-mode=plan", "-c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            resume_line(&claude, &argv).unwrap(),
            "claude --resume 08dbf0dd-5f37-4bb8-b4ed-dc8ec1731037 --model opus --dangerously-skip-permissions --permission-mode=plan"
        );
        let codex = Agent { kind: AgentKind::Codex, session: "5973b6c0-94b8-487b-a530-2aeb6098ae0e".into() };
        assert_eq!(resume_line(&codex, &[]).unwrap(), "codex resume 5973b6c0-94b8-487b-a530-2aeb6098ae0e");
        assert!(resume_line(&Agent { kind: AgentKind::Claude, session: "x; rm -rf ~".into() }, &[]).is_none());

        let mut p = PaneRec { agent: Some(claude), command: vec!["claude".into()], ..Default::default() };
        assert_eq!(restore_line(&p).map(|(_, run)| run), Some(true));
        p.agent = None;
        p.command = vec!["npm".into(), "run".into(), "dev".into()];
        assert_eq!(restore_line(&p), Some(("npm run dev".into(), false)));
        p.command = vec!["echo".into(), "a\nrm -rf ~".into()];
        assert_eq!(restore_line(&p), None);
        p.command.clear();
        assert_eq!(restore_line(&p), None);
    }

    #[test]
    fn reads_what_is_running() {
        assert_eq!(retitled(vec!["npm run dev".into()]), ["npm", "run", "dev"]);
        assert_eq!(retitled(vec!["vim".into(), "a b.txt".into()]), ["vim", "a b.txt"]);
        let p = Path::new("/home/u/.codex/sessions/2026/11/24/rollout-2026-11-24T10-02-03-5973b6c0-94b8-487b-a530-2aeb6098ae0e.jsonl");
        assert_eq!(rollout_id(p).as_deref(), Some("5973b6c0-94b8-487b-a530-2aeb6098ae0e"));
        assert_eq!(rollout_id(Path::new("/tmp/rollout-x.jsonl")), None);
        // This test process: not an agent, and its own command line comes back.
        let (argv, agent) = probe(std::process::id());
        assert!(!argv.is_empty() && agent.is_none());
        assert_eq!(clean_title("✳ Fix the login bug").as_deref(), Some("Fix the login bug"));
        assert_eq!(clean_title("me@box:~/s-term"), None);
        assert_eq!(clean_title("shell"), None);
    }

    /// Needs a Claude Code running on this machine: its pane would resume the same conversation.
    #[test]
    #[ignore]
    fn live_probe_finds_running_claude() {
        let pids = std::fs::read_dir("/proc").unwrap().flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok());
        let claude = pids.filter(|&p| std::fs::read_to_string(format!("/proc/{p}/comm")).is_ok_and(|c| c.trim_end() == "claude")).find(|&p| claude_session(p).is_some());
        let pid = claude.expect("no running Claude Code with a sessions/<pid>.json");
        let (argv, agent) = probe(pid);
        let agent = agent.expect("agent found");
        assert_eq!(agent.kind, AgentKind::Claude);
        let line = restore_line(&PaneRec { command: argv.clone(), agent: Some(agent.clone()), ..Default::default() }).unwrap();
        eprintln!("pid {pid}: {argv:?} -> {line:?}");
        assert_eq!(line, (format!("claude --resume {}", agent.session), true));
    }

    #[test]
    fn describes_entries() {
        let mut p = pane("/srv/web", &["npm", "run", "dev"]);
        assert_eq!(entry(Kind::Tab, 0, vec![p.clone()]).describe(), ("web".into(), "npm run dev".into()));
        p.agent = Some(Agent { kind: AgentKind::Claude, session: "ab-12".into() });
        p.title = "⠂ Fix the login bug".into();
        assert_eq!(entry(Kind::Pane, 0, vec![p.clone()]).describe().1, "Claude Code · Fix the login bug");
        let split = entry(Kind::Tab, 0, vec![p, pane("/srv/api", &["cargo", "run"]), pane("/srv/web", &[])]);
        assert_eq!(split.describe(), ("web + api".into(), "3 panes · claude, cargo".into()));
    }

    /// Drives real egui frames: a click on a row reopens it, its × deletes it.
    #[test]
    fn row_click_restores_and_cross_deletes() {
        use egui::{Event, PointerButton, RawInput};
        let ctx = egui::Context::default();
        let theme = Theme::preset("graphite");
        let mut hist = History::open(None);
        hist.push(entry(Kind::Tab, now(), vec![pane("/srv/web", &["make"])]));
        let id = hist.entries[0].id;
        let mut st = HistoryUi::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        let mut frame = |events: Vec<Event>| {
            let mut actions = Vec::new();
            let input = RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| panel(ui, &theme, &mut st, &hist, &mut actions));
            });
            actions.into_iter().filter_map(|a| if let UiAction::History(id, op) = a { Some((id, op)) } else { None }).collect::<Vec<_>>()
        };
        frame(vec![]);
        let at = |rid: Id| ctx.read_response(rid).expect("drawn").rect.center();
        let click = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        let row_id = Id::new(("s-term-hist", id));
        let (row, del) = (at(row_id) - vec2(60.0, 0.0), at(row_id.with("del")));
        frame(vec![Event::PointerMoved(row)]);
        frame(vec![click(row, true)]);
        assert_eq!(frame(vec![click(row, false)]), [(id, Op::Restore)]);
        frame(vec![Event::PointerMoved(del)]);
        frame(vec![click(del, true)]);
        assert_eq!(frame(vec![click(del, false)]), [(id, Op::Delete)]);
    }
}

//! The other S-term windows. Each window is its own process. It publishes the AI agents running in
//! its panes to `<dir>/<pid>.json` (`dir` is `$XDG_RUNTIME_DIR/s-term/windows`), so every window's
//! AI view lists the agents of all of them, and listens on `<dir>/<pid>.sock` for `focus <pane>`,
//! sent by a window whose list was clicked. A thread rewrites this window's file when its agents
//! change, reads the others' every 400 ms, and deletes the files of windows that are gone.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::usage_ui::AgentPane;

const POLL: Duration = Duration::from_millis(400);

/// An agent pane as a window's file has it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Wire {
    pane: u64,
    tool: String,
    pgid: u32,
    tab: usize,
    title: String,
}

impl Wire {
    fn of(a: &AgentPane) -> Self {
        Wire { pane: a.pane, tool: a.tool.to_string(), pgid: a.pgid, tab: a.tab, title: a.title.clone() }
    }

    /// The pane, as an agent of window `pid`. Only the tools S-term knows are kept.
    fn pane_of(&self, pid: u32) -> Option<AgentPane> {
        let tool = ["claude", "codex", "agy"].into_iter().find(|t| *t == self.tool)?;
        Some(AgentPane { window: Some(pid), pane: self.pane, tool, pgid: self.pgid, tab: self.tab, title: self.title.chars().take(300).collect() })
    }
}

pub struct Peers {
    /// None when off (screenshot mode) or when the folder can't be made.
    dir: Option<PathBuf>,
    mine: Arc<Mutex<Vec<Wire>>>,
    others: Arc<Mutex<Arc<Vec<AgentPane>>>>,
    focus: Receiver<u64>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn windows_dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()).map(PathBuf::from);
    let base = runtime.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("s-term/windows"))
}

fn comm(pid: &str) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|c| c.trim_end().to_string())
}

/// Writes `bytes` to `path` through a temporary file, readable only by the user (titles and
/// folders are private).
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?.write_all(bytes)?;
    std::fs::rename(tmp, path)
}

impl Peers {
    /// `on` is false in screenshot mode: then nothing is published, read or listened to.
    pub fn new(ctx: egui::Context, on: bool) -> Self {
        let (tx, focus) = channel();
        let mine: Arc<Mutex<Vec<Wire>>> = Arc::default();
        let others: Arc<Mutex<Arc<Vec<AgentPane>>>> = Arc::default();
        let dir = windows_dir().filter(|d| on && std::fs::DirBuilder::new().recursive(true).mode(0o700).create(d).is_ok());
        if let Some(d) = &dir {
            let me = std::process::id();
            let sock = d.join(format!("{me}.sock"));
            // A file left by an earlier process with this pid.
            let _ = std::fs::remove_file(&sock);
            if let Ok(listener) = UnixListener::bind(&sock) {
                let ctx = ctx.clone();
                let _ = std::thread::Builder::new().name("s-term-peers-listen".into()).spawn(move || {
                    for stream in listener.incoming().flatten() {
                        if let Some(pane) = read_request(stream) {
                            if tx.send(pane).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                    }
                });
            }
            let (d, m, o) = (d.clone(), mine.clone(), others.clone());
            let _ = std::thread::Builder::new().name("s-term-peers".into()).spawn(move || poll(d, me, m, o, ctx));
        }
        Self { dir, mine, others, focus }
    }

    /// This window's agent panes, for the other windows.
    pub fn set_mine(&self, agents: &[AgentPane]) {
        *lock(&self.mine) = agents.iter().map(Wire::of).collect();
    }

    /// The agent panes of the other windows: the oldest window first, each in tab order.
    pub fn others(&self) -> Arc<Vec<AgentPane>> {
        lock(&self.others).clone()
    }

    /// A pane another window asked this one to show.
    pub fn focus_request(&self) -> Option<u64> {
        self.focus.try_recv().ok()
    }

    /// Asks window `pid` to show its pane `pane`.
    pub fn focus(&self, pid: u32, pane: u64) {
        let Some(sock) = self.dir.as_ref().map(|d| d.join(format!("{pid}.sock"))) else { return };
        let _ = std::thread::Builder::new().name("s-term-peers-send".into()).spawn(move || {
            if let Ok(mut s) = UnixStream::connect(sock) {
                let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
                let _ = s.write_all(format!("focus {pane}\n").as_bytes());
            }
        });
    }
}

impl Drop for Peers {
    fn drop(&mut self) {
        if let Some(d) = &self.dir {
            let me = std::process::id();
            let _ = std::fs::remove_file(d.join(format!("{me}.json")));
            let _ = std::fs::remove_file(d.join(format!("{me}.sock")));
        }
    }
}

/// `focus <pane>`, and nothing else.
fn read_request(mut s: UnixStream) -> Option<u64> {
    let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
    let mut buf = [0u8; 64];
    let mut n = 0;
    while n < buf.len() {
        match s.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
        if buf[..n].contains(&b'\n') {
            break;
        }
    }
    parse_request(std::str::from_utf8(&buf[..n]).ok()?)
}

fn parse_request(s: &str) -> Option<u64> {
    s.trim_end().strip_prefix("focus ")?.parse().ok()
}

fn poll(dir: PathBuf, me: u32, mine: Arc<Mutex<Vec<Wire>>>, others: Arc<Mutex<Arc<Vec<AgentPane>>>>, ctx: egui::Context) {
    let own = dir.join(format!("{me}.json"));
    // Another window is one whose process has this one's name (a pid can be reused by anything).
    let name = comm("self").unwrap_or_else(|| "s-term".into());
    let mut written: Option<Vec<Wire>> = None;
    let mut read: HashMap<u32, (SystemTime, Vec<Wire>)> = HashMap::new();
    loop {
        let cur = lock(&mine).clone();
        if written.as_ref() != Some(&cur) && write_atomic(&own, &serde_json::to_vec(&cur).unwrap_or_default()).is_ok() {
            written = Some(cur);
        }
        let mut seen = Vec::new();
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            let Some(pid) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) else { continue };
            if pid == me {
                continue;
            }
            if comm(&pid.to_string()).as_ref() != Some(&name) {
                // The window is gone: its `.json` and `.sock` (and any `.json.tmp`) go too.
                let _ = std::fs::remove_file(&p);
                continue;
            }
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            seen.push(pid);
            let Ok(t) = e.metadata().and_then(|m| m.modified()) else { continue };
            if read.get(&pid).is_some_and(|(at, _)| *at == t) {
                continue;
            }
            if let Some(v) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Vec<Wire>>(&b).ok()) {
                read.insert(pid, (t, v));
            }
        }
        read.retain(|pid, _| seen.contains(pid));
        let mut pids: Vec<u32> = read.keys().copied().collect();
        pids.sort_unstable();
        let list: Vec<AgentPane> = pids.iter().flat_map(|pid| read[pid].1.iter().filter_map(|w| w.pane_of(*pid))).collect();
        let mut shown = lock(&others);
        if **shown != list {
            *shown = Arc::new(list);
            ctx.request_repaint();
        }
        drop(shown);
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_files() {
        assert_eq!(parse_request("focus 12\n"), Some(12));
        assert_eq!(parse_request("focus 12"), Some(12));
        assert_eq!(parse_request("focus -1\n"), None);
        assert_eq!(parse_request("focus 1; rm -rf\n"), None);
        assert_eq!(parse_request("close 3\n"), None);
        let w = Wire { pane: 4, tool: "claude".into(), pgid: 99, tab: 1, title: "✳ Fix it".into() };
        let back: Vec<Wire> = serde_json::from_slice(&serde_json::to_vec(&vec![w.clone()]).unwrap()).unwrap();
        let a = back[0].pane_of(7).unwrap();
        assert_eq!((a.window, a.pane, a.tool, a.pgid, a.tab), (Some(7), 4, "claude", 99, 1));
        assert!(Wire { tool: "vim".into(), ..w }.pane_of(7).is_none());
    }
}

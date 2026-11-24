//! Side panel: git changes, file tree and a diff/file viewer for the focused pane's folder.
//!
//! All filesystem and git work happens on a background thread that refreshes about once a
//! second; the UI only reads the latest snapshot, so it never blocks rendering.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Rect, RichText, Sense};

use crate::theme::{c32, lerp_color, Theme};

const CLICK: Sense = Sense { click: true, drag: false, focusable: false };
const ROW_H: f32 = 22.0;
const RECENT_SECS: f32 = 90.0;
const MAX_CHANGES: usize = 500;
const MAX_DIR_ENTRIES: usize = 2000;
const MAX_VIEW_LINES: usize = 20_000;
const MAX_PREVIEW_BYTES: u64 = 512 * 1024;
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// Larger images are downscaled before upload (GPU texture limits, memory).
const MAX_IMAGE_SIDE: u32 = 4096;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "tif", "tiff", "avif", "heic"];
/// The subset we can decode for the viewer.
const DECODABLE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"];

fn ext_in(p: &Path, list: &[&str]) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| list.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub fn is_image(p: &Path) -> bool {
    ext_in(p, IMAGE_EXTS)
}

// ------------------------------------------------------------------------------------------ data

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
    Other,
}

impl ChangeKind {
    fn from_xy(x: char, y: char) -> Self {
        if x == '?' {
            return Self::Untracked;
        }
        if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
            return Self::Conflict;
        }
        match if x != ' ' { x } else { y } {
            'M' | 'T' => Self::Modified,
            'A' => Self::Added,
            'D' => Self::Deleted,
            'R' | 'C' => Self::Renamed,
            _ => Self::Other,
        }
    }

    fn badge(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "U",
            Self::Conflict => "!",
            Self::Other => "•",
        }
    }

    fn color(self, t: &Theme) -> Color32 {
        c32(match self {
            Self::Modified => t.palette[3],
            Self::Added | Self::Untracked => t.palette[2],
            Self::Deleted | Self::Conflict => t.palette[1],
            Self::Renamed => t.palette[4],
            Self::Other => t.palette[8],
        })
    }
}

pub struct Change {
    /// Repo-relative path.
    pub rel: String,
    pub kind: ChangeKind,
    pub adds: Option<u32>,
    pub dels: Option<u32>,
    pub mtime: Option<SystemTime>,
}

pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub when: String,
    pub date: String,
    pub author: String,
    pub email: String,
    pub coauthors: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Gemini,
}

impl Commit {
    /// AI agent that wrote or co-wrote this commit, from the author or Co-authored-by trailers.
    pub fn agent(&self) -> Option<Agent> {
        let who = std::iter::once(format!("{} {}", self.author, self.email)).chain(self.coauthors.iter().cloned());
        let mut found = None;
        for w in who {
            let w = w.to_lowercase();
            if w.contains("claude") || w.contains("anthropic") {
                return Some(Agent::Claude);
            }
            if w.contains("gemini") || w.contains("antigravity") {
                found = Some(Agent::Gemini);
            }
        }
        found
    }
}

pub struct GitInfo {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<Change>,
    pub truncated: bool,
    pub commits: Vec<Commit>,
    /// Absolute paths of ignored files/dirs (dimmed in the tree).
    pub ignored: HashSet<PathBuf>,
    /// Absolute path -> change kind (colors the tree).
    pub by_path: HashMap<PathBuf, ChangeKind>,
}

pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub mtime: Option<SystemTime>,
}

pub struct Snapshot {
    /// The folder being followed (focused pane's cwd).
    pub dir: PathBuf,
    /// Repo root, or `dir` when not in a repo.
    pub root: PathBuf,
    pub git: Option<GitInfo>,
    pub tree: HashMap<PathBuf, Vec<Entry>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Meta,
    Hunk,
    Add,
    Del,
    Context,
}

pub struct FileView {
    pub title: String,
    pub abs: PathBuf,
    pub is_diff: bool,
    pub lines: Vec<(LineKind, String)>,
    pub max_len: usize,
    pub adds: u32,
    pub dels: u32,
    pub note: Option<String>,
    /// Decoded image (image files only); uploaded to a texture by the UI.
    pub image: Option<egui::ColorImage>,
}

// ---------------------------------------------------------------------------------------- worker

/// What the worker keeps refreshing.
#[derive(Clone, Default, PartialEq)]
struct Watch {
    /// The followed folder (focused pane's cwd).
    dir: Option<PathBuf>,
    /// Expanded folders, across every Explorer section.
    expanded: Vec<PathBuf>,
    /// Extra Explorer folders (each shown as its own section).
    extras: Vec<PathBuf>,
    show_hidden: bool,
}

enum Req {
    Watch(Watch),
    Diff { root: PathBuf, rel: String, untracked: bool },
    Preview(PathBuf, String),
}

fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        // Never take index.lock: don't fight with git commands the agent runs.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

fn nul_split(raw: &[u8]) -> Vec<String> {
    raw.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

fn parse_branch(h: &str) -> (String, u32, u32) {
    let (head, track) = match h.split_once(" [") {
        Some((a, b)) => (a, b.trim_end_matches(']')),
        None => (h, ""),
    };
    let head = head.strip_prefix("No commits yet on ").unwrap_or(head);
    let branch = head.split("...").next().unwrap_or(head).to_string();
    let mut ahead = 0;
    let mut behind = 0;
    for part in track.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.parse().unwrap_or(0);
        }
    }
    (branch, ahead, behind)
}

fn numstat(root: &Path, args: &[&str], out: &mut HashMap<String, (Option<u32>, Option<u32>)>) -> bool {
    let Some(raw) = git(root, args) else { return false };
    let items = nul_split(&raw);
    let mut i = 0;
    while i < items.len() {
        let mut f = items[i].splitn(3, '\t');
        let a: Option<u32> = f.next().and_then(|x| x.parse().ok());
        let d: Option<u32> = f.next().and_then(|x| x.parse().ok());
        let mut path = f.next().unwrap_or("").to_string();
        if path.is_empty() {
            // Rename: "a\td\t\0old\0new"
            path = items.get(i + 2).cloned().unwrap_or_default();
            i += 2;
        }
        let e = out.entry(path).or_insert((Some(0), Some(0)));
        e.0 = e.0.zip(a).map(|(x, y)| x + y);
        e.1 = e.1.zip(d).map(|(x, y)| x + y);
        i += 1;
    }
    true
}

fn count_lines(p: &Path) -> Option<u32> {
    let meta = std::fs::metadata(p).ok()?;
    if !meta.is_file() || meta.len() > MAX_PREVIEW_BYTES {
        return None;
    }
    let bytes = std::fs::read(p).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let n = bytes.iter().filter(|&&b| b == b'\n').count() as u32;
    Some(if bytes.last().is_some_and(|&b| b != b'\n') { n + 1 } else { n })
}

fn git_info(root: &Path) -> Option<GitInfo> {
    let raw = git(root, &["status", "--porcelain=v1", "-z", "-b", "--untracked-files=all"])?;
    let items = nul_split(&raw);
    let (mut branch, mut ahead, mut behind) = (String::from("?"), 0, 0);
    let mut entries = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let s = &items[i];
        if let Some(h) = s.strip_prefix("## ") {
            (branch, ahead, behind) = parse_branch(h);
        } else if s.len() > 3 {
            let mut cs = s.chars();
            let (x, y) = (cs.next().unwrap_or(' '), cs.next().unwrap_or(' '));
            entries.push((ChangeKind::from_xy(x, y), s[3..].to_string()));
            if x == 'R' || x == 'C' {
                i += 1; // skip the old path
            }
        }
        i += 1;
    }

    let mut stats = HashMap::new();
    if !numstat(root, &["diff", "--numstat", "-z", "HEAD"], &mut stats) {
        numstat(root, &["diff", "--numstat", "-z", "--cached"], &mut stats);
        numstat(root, &["diff", "--numstat", "-z"], &mut stats);
    }

    let truncated = entries.len() > MAX_CHANGES;
    let mut by_path = HashMap::new();
    let mut changes: Vec<Change> = entries
        .into_iter()
        .take(MAX_CHANGES)
        .map(|(kind, rel)| {
            let abs = root.join(&rel);
            by_path.insert(abs.clone(), kind);
            let (adds, dels) = match stats.get(&rel) {
                Some(&(a, d)) => (a, d),
                None if kind == ChangeKind::Untracked => (count_lines(&abs), Some(0)),
                None => (None, None),
            };
            let mtime = std::fs::metadata(&abs).and_then(|m| m.modified()).ok();
            Change { rel, kind, adds, dels, mtime }
        })
        .collect();
    // Most recently touched first: that's what you want to see while an agent works.
    changes.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.rel.cmp(&b.rel)));

    let fmt = "--format=%h%x1f%s%x1f%cr%x1f%cd%x1f%an%x1f%ae%x1f%(trailers:key=Co-authored-by,valueonly,separator=%x1e)";
    let commits = git(root, &["log", "-10", "-z", "--date=format:%Y-%m-%d %H:%M", fmt])
        .map(|raw| nul_split(&raw).iter().filter_map(|c| parse_commit(c)).collect())
        .unwrap_or_default();

    let ignored = git(root, &["ls-files", "-z", "-o", "-i", "--exclude-standard", "--directory"])
        .map(|raw| nul_split(&raw).into_iter().map(|p| root.join(p.trim_end_matches('/'))).collect())
        .unwrap_or_default();

    Some(GitInfo { branch, ahead, behind, changes, truncated, commits, ignored, by_path })
}

fn parse_commit(c: &str) -> Option<Commit> {
    let mut f = c.trim_start_matches('\n').splitn(7, '\x1f');
    Some(Commit {
        hash: f.next()?.into(),
        subject: f.next()?.into(),
        when: f.next()?.into(),
        date: f.next()?.into(),
        author: f.next()?.into(),
        email: f.next()?.into(),
        coauthors: f.next().unwrap_or("").split('\x1e').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
    })
}

/// `46 minutes ago` -> `46m`, `3 days ago` -> `3d`.
fn short_age(rel: &str) -> String {
    let mut it = rel.split_whitespace();
    let (Some(n), Some(unit)) = (it.next(), it.next()) else { return rel.to_string() };
    let u = match unit.trim_end_matches(',') {
        u if u.starts_with("second") => "s",
        u if u.starts_with("minute") => "m",
        u if u.starts_with("hour") => "h",
        u if u.starts_with("day") => "d",
        u if u.starts_with("week") => "w",
        u if u.starts_with("month") => "mo",
        u if u.starts_with("year") => "y",
        _ => return rel.to_string(),
    };
    format!("{n}{u}")
}

/// Stable color per author.
fn author_color(name: &str) -> Color32 {
    let h = name.bytes().fold(2166136261u32, |a, b| (a ^ b as u32).wrapping_mul(16777619));
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.45, 0.85, 1.0).into()
}

fn initials(name: &str) -> String {
    let mut it = name.split_whitespace().filter_map(|w| w.chars().next());
    let a = it.next().unwrap_or('?');
    match it.last() {
        Some(b) => format!("{}{}", a.to_uppercase(), b.to_uppercase()),
        None => a.to_uppercase().collect(),
    }
}

fn list_dir(dir: &Path, show_hidden: bool) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Entry> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() != ".git" && (show_hidden || !e.file_name().to_string_lossy().starts_with('.')))
        .take(MAX_DIR_ENTRIES)
        .map(|e| {
            let meta = std::fs::metadata(e.path()).ok();
            Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path(),
                is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                mtime: meta.and_then(|m| m.modified().ok()),
            }
        })
        .collect();
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    v
}

fn repo_root(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"]).map(|o| PathBuf::from(String::from_utf8_lossy(&o).trim())).filter(|p| !p.as_os_str().is_empty())
}

fn list_tree(root: &Path, expanded: &[PathBuf], show_hidden: bool) -> HashMap<PathBuf, Vec<Entry>> {
    let mut tree = HashMap::new();
    tree.insert(root.to_path_buf(), list_dir(root, show_hidden));
    for e in expanded.iter().filter(|e| e.starts_with(root) && *e != root) {
        tree.insert(e.clone(), list_dir(e, show_hidden));
    }
    tree
}

/// The followed folder: its tree starts at the repository root.
fn build_snapshot(dir: &Path, expanded: &[PathBuf], show_hidden: bool) -> Snapshot {
    let repo = repo_root(dir);
    let git = repo.as_deref().and_then(git_info);
    let root = repo.unwrap_or_else(|| dir.to_path_buf());
    let tree = list_tree(&root, expanded, show_hidden);
    Snapshot { dir: dir.to_path_buf(), root, git, tree }
}

/// An extra Explorer folder: its tree starts at the folder itself, colored by its repo's status.
fn build_folder(root: &Path, expanded: &[PathBuf], show_hidden: bool) -> Snapshot {
    let git = repo_root(root).as_deref().and_then(git_info);
    let tree = list_tree(root, expanded, show_hidden);
    Snapshot { dir: root.to_path_buf(), root: root.to_path_buf(), git, tree }
}

fn classify(lines: impl Iterator<Item = String>, diff: bool) -> FileView {
    let mut out = Vec::new();
    let (mut adds, mut dels, mut max_len) = (0, 0, 0);
    for l in lines.take(MAX_VIEW_LINES) {
        let l = l.replace('\t', "    ");
        let kind = if !diff {
            LineKind::Context
        } else if l.starts_with("+++") || l.starts_with("---") || l.starts_with("diff ") || l.starts_with("index ") {
            LineKind::Meta
        } else if l.starts_with("@@") {
            LineKind::Hunk
        } else if l.starts_with('+') {
            adds += 1;
            LineKind::Add
        } else if l.starts_with('-') {
            dels += 1;
            LineKind::Del
        } else {
            LineKind::Context
        };
        max_len = max_len.max(l.chars().count());
        out.push((kind, l));
    }
    FileView { title: String::new(), abs: PathBuf::new(), is_diff: diff, lines: out, max_len, adds, dels, note: None, image: None }
}

fn load_text(abs: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(abs).map_err(|e| e.to_string())?;
    if meta.len() > MAX_PREVIEW_BYTES {
        return Err(format!("File is large ({} KB); open it in an editor.", meta.len() / 1024));
    }
    let bytes = std::fs::read(abs).map_err(|e| e.to_string())?;
    if bytes.contains(&0) {
        return Err("Binary file.".into());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn load_diff(root: &Path, rel: &str, untracked: bool) -> FileView {
    let abs = root.join(rel);
    let mut view = if untracked {
        match load_text(&abs) {
            Ok(t) => {
                let header = [format!("new file: {rel}"), format!("@@ +1,{} @@", t.lines().count())];
                classify(header.into_iter().chain(t.lines().map(|l| format!("+{l}"))), true)
            }
            Err(e) => FileView { note: Some(e), ..classify(std::iter::empty(), true) },
        }
    } else {
        let text = [&["diff", "--no-color", "HEAD", "--", rel][..], &["diff", "--no-color", "--cached", "--", rel], &["diff", "--no-color", "--", rel]]
            .iter()
            .filter_map(|args| git(root, args))
            .map(|o| String::from_utf8_lossy(&o).into_owned())
            .find(|s| !s.trim().is_empty())
            .unwrap_or_default();
        let mut v = classify(text.lines().map(String::from), true);
        if v.lines.is_empty() {
            v.note = Some("No textual diff (binary file, or mode change only).".into());
        }
        v
    };
    view.title = rel.to_string();
    view.abs = abs;
    view
}

fn load_image(abs: &Path) -> Result<(egui::ColorImage, String), String> {
    let meta = std::fs::metadata(abs).map_err(|e| e.to_string())?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!("Image is large ({} MB); open it externally.", meta.len() / (1024 * 1024)));
    }
    let img = image::ImageReader::open(abs)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| format!("Can't decode image: {e}"))?;
    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > MAX_IMAGE_SIDE { img.thumbnail(MAX_IMAGE_SIDE, MAX_IMAGE_SIDE) } else { img };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let kb = meta.len() / 1024;
    let info = if kb >= 1024 { format!("{w}×{h} · {:.1} MB", kb as f32 / 1024.0) } else { format!("{w}×{h} · {kb} KB") };
    Ok((egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), info))
}

fn load_preview(abs: &Path, title: String) -> FileView {
    if ext_in(abs, DECODABLE_EXTS) {
        let mut view = classify(std::iter::empty(), false);
        match load_image(abs) {
            Ok((img, info)) => {
                view.image = Some(img);
                view.note = Some(info);
            }
            Err(e) => view.note = Some(e),
        }
        view.title = title;
        view.abs = abs.to_path_buf();
        return view;
    }
    let mut view = match load_text(abs) {
        Ok(t) => classify(t.lines().map(String::from), false),
        Err(e) => FileView { note: Some(e), ..classify(std::iter::empty(), false) },
    };
    view.title = title;
    view.abs = abs.to_path_buf();
    view
}

// --------------------------------------------------------------------------------------- UI state

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Changes,
    Files,
    Ai,
}

/// Things the panel wants the app to do.
pub enum UiAction {
    /// Type text into the focused terminal.
    Insert(String),
    Copy(String),
    OpenExternal(PathBuf),
    /// A file was dragged out of the panel and released at this screen position.
    Drop(PathBuf, egui::Pos2),
}

pub struct Sidebar {
    pub open: bool,
    tab: Tab,
    expanded: HashSet<PathBuf>,
    /// Expanded folders inside the extra Explorer sections (kept when the followed project changes).
    extra_expanded: HashSet<PathBuf>,
    /// Extra Explorer sections folded down to their header.
    collapsed: HashSet<PathBuf>,
    /// Relative heights of the Explorer panes, keyed by folder (the followed project is the empty path).
    pane_weights: HashMap<PathBuf, f32>,
    /// Last request sent to the worker.
    sent: Watch,
    extra_snaps: Arc<Mutex<Vec<Arc<Snapshot>>>>,
    /// "Add folder" field, while open.
    adding: Option<String>,
    /// Folder picked in the system dialog, delivered by its thread.
    picked: Arc<Mutex<Option<PathBuf>>>,
    watching: Option<PathBuf>,
    watched_root: Option<PathBuf>,
    last_follow: Instant,
    tx: Sender<Req>,
    snap: Arc<Mutex<Option<Arc<Snapshot>>>>,
    loaded_view: Arc<Mutex<Option<FileView>>>,
    pub viewer: Option<FileView>,
    viewer_loading: bool,
    viewer_tex: Option<egui::TextureHandle>,
    /// File being dragged out of the panel (towards a pane).
    dragging: Option<PathBuf>,
    pub usage: crate::usage::Usage,
    usage_ui: crate::usage_ui::UsageUi,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn ago(t: Option<SystemTime>) -> (String, f32) {
    let Some(secs) = t.and_then(|t| SystemTime::now().duration_since(t).ok()).map(|d| d.as_secs_f32()) else {
        return (String::new(), f32::MAX);
    };
    let s = match secs as u64 {
        0..=4 => "now".to_string(),
        s @ 5..=59 => format!("{s}s"),
        s @ 60..=3599 => format!("{}m", s / 60),
        s @ 3600..=86399 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    };
    (s, secs)
}

/// Shell-safe-ish path for typing into a prompt.
pub fn quote(p: &str) -> String {
    if p.chars().any(|c| c.is_whitespace() || "'\"$`\\()&;|<>*?".contains(c)) {
        format!("'{}'", p.replace('\'', "'\\''"))
    } else {
        p.to_string()
    }
}

const EXTRA_HEAD_H: f32 = 30.0;
/// Smallest height an Explorer pane can be dragged down to.
const PANE_MIN_H: f32 = 60.0;

/// What a click on an extra folder's header asks for.
#[derive(Default)]
struct FolderHead {
    toggle: bool,
    remove: bool,
    action: Option<UiAction>,
}

/// Everything one Explorer tree needs while it is drawn, and what it asks for.
struct TreeCx<'a> {
    theme: &'a Theme,
    snap: &'a Snapshot,
    /// Typed paths are relative to this folder (the focused pane's).
    base: &'a Path,
    expanded: &'a HashSet<PathBuf>,
    hide_ignored: bool,
    actions: &'a mut Vec<UiAction>,
    dragging: &'a mut Option<PathBuf>,
    toggled: Option<PathBuf>,
    preview: Option<(PathBuf, String)>,
    /// A folder to add as its own Explorer section.
    pin: Option<PathBuf>,
}

impl<'a> TreeCx<'a> {
    fn new(
        theme: &'a Theme,
        snap: &'a Snapshot,
        base: &'a Path,
        expanded: &'a HashSet<PathBuf>,
        hide_ignored: bool,
        actions: &'a mut Vec<UiAction>,
        dragging: &'a mut Option<PathBuf>,
    ) -> Self {
        Self { theme, snap, base, expanded, hide_ignored, actions, dragging, toggled: None, preview: None, pin: None }
    }
}

impl Sidebar {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel::<Req>();
        let snap: Arc<Mutex<Option<Arc<Snapshot>>>> = Arc::default();
        let loaded_view: Arc<Mutex<Option<FileView>>> = Arc::default();
        let usage = crate::usage::Usage::new(ctx.clone());
        let extra_snaps: Arc<Mutex<Vec<Arc<Snapshot>>>> = Arc::default();
        let (snap2, view2, extra2) = (snap.clone(), loaded_view.clone(), extra_snaps.clone());
        std::thread::Builder::new()
            .name("s-term-sidebar".into())
            .spawn(move || {
                let mut w = Watch::default();
                loop {
                    match rx.recv_timeout(Duration::from_millis(1200)) {
                        Ok(Req::Watch(new)) => w = new,
                        Ok(Req::Diff { root, rel, untracked }) => {
                            *lock(&view2) = Some(load_diff(&root, &rel, untracked));
                            ctx.request_repaint();
                            continue;
                        }
                        Ok(Req::Preview(p, title)) => {
                            *lock(&view2) = Some(load_preview(&p, title));
                            ctx.request_repaint();
                            continue;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    if let Some(d) = &w.dir {
                        let s = build_snapshot(d, &w.expanded, w.show_hidden);
                        *lock(&snap2) = Some(Arc::new(s));
                        ctx.request_repaint();
                    }
                    let extras: Vec<Arc<Snapshot>> = w.extras.iter().filter(|f| f.is_dir()).map(|f| Arc::new(build_folder(f, &w.expanded, w.show_hidden))).collect();
                    if !extras.is_empty() || !lock(&extra2).is_empty() {
                        *lock(&extra2) = extras;
                        ctx.request_repaint();
                    }
                }
            })
            .expect("spawn sidebar thread");

        Self {
            open: false,
            tab: Tab::Changes,
            expanded: HashSet::new(),
            extra_expanded: HashSet::new(),
            collapsed: HashSet::new(),
            pane_weights: HashMap::new(),
            sent: Watch::default(),
            extra_snaps,
            adding: None,
            picked: Arc::default(),
            watching: None,
            watched_root: None,
            last_follow: Instant::now() - Duration::from_secs(10),
            tx,
            snap,
            loaded_view,
            viewer: None,
            viewer_loading: false,
            viewer_tex: None,
            dragging: None,
            usage,
            usage_ui: Default::default(),
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn view(&self) -> Tab {
        self.tab
    }

    /// Rail click: open on `tab`, or close if it is already showing.
    pub fn toggle_view(&mut self, tab: Tab) {
        if self.open && self.tab == tab {
            self.open = false;
        } else {
            self.open = true;
            self.tab = tab;
        }
    }

    pub fn show_view(&mut self, tab: Tab) {
        self.open = true;
        self.tab = tab;
    }

    /// Latest git/files snapshot of the followed folder (also used by the status bar).
    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        lock(&self.snap).clone().filter(|s| Some(&s.dir) == self.watching.as_ref())
    }

    pub fn follow(&mut self, cwd: Option<PathBuf>) {
        self.last_follow = Instant::now();
        self.usage.set_focus(cwd.clone());
        if cwd.is_some() && cwd != self.watching {
            self.watching = cwd;
            self.send_watch();
        }
    }

    fn send_watch(&mut self) {
        let w = Watch {
            dir: self.watching.clone(),
            expanded: self.expanded.iter().chain(&self.extra_expanded).cloned().collect(),
            ..self.sent.clone()
        };
        self.sent = w.clone();
        let _ = self.tx.send(Req::Watch(w));
    }

    /// Re-sends the request when the Explorer settings changed.
    fn sync_settings(&mut self, cfg: &crate::config::Config) {
        if self.sent.extras != cfg.explorer_folders || self.sent.show_hidden != cfg.show_hidden {
            self.sent.extras = cfg.explorer_folders.clone();
            self.sent.show_hidden = cfg.show_hidden;
            self.send_watch();
        }
    }

    /// Opens the system folder picker (zenity) on a background thread.
    fn browse(&self) {
        let picked = self.picked.clone();
        let _ = std::thread::Builder::new().name("s-term-pick".into()).spawn(move || {
            let out = Command::new("zenity").args(["--file-selection", "--directory", "--title=Add folder to Explorer"]).stderr(Stdio::null()).output();
            if let Some(o) = out.ok().filter(|o| o.status.success()) {
                let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !p.is_empty() {
                    *lock(&picked) = Some(PathBuf::from(p));
                }
            }
        });
    }

    /// Starts adding an Explorer folder (panel button, command palette).
    pub fn start_adding(&mut self) {
        self.show_view(Tab::Files);
        self.adding.get_or_insert_with(String::new);
    }

    fn open_diff(&mut self, root: &Path, c: &Change) {
        self.viewer_loading = true;
        let untracked = c.kind == ChangeKind::Untracked;
        let _ = self.tx.send(Req::Diff { root: root.to_path_buf(), rel: c.rel.clone(), untracked });
    }

    /// The file currently being dragged out of the panel, if any.
    pub fn dragging(&self) -> Option<&Path> {
        self.dragging.as_deref()
    }

    /// Makes a row a drag source: dragging it carries `path`; releasing emits `UiAction::Drop`.
    fn drag_source(dragging: &mut Option<PathBuf>, resp: &egui::Response, path: &Path, actions: &mut Vec<UiAction>) {
        if resp.drag_started() {
            *dragging = Some(path.to_path_buf());
        }
        if resp.drag_stopped() {
            *dragging = None;
            if let Some(pos) = resp.ctx.input(|i| i.pointer.latest_pos()) {
                actions.push(UiAction::Drop(path.to_path_buf(), pos));
            }
        }
    }

    /// Floating chip that follows the pointer while a file is dragged.
    fn drag_chip(&self, ctx: &egui::Context, theme: &Theme) {
        let (Some(path), Some(pos)) = (&self.dragging, ctx.pointer_latest_pos()) else { return };
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("s-term-drag-chip")));
        let g = p.layout_no_wrap(name, FontId::monospace(12.0), theme.text);
        let r = Rect::from_min_size(pos + vec2(14.0, 10.0), g.size() + vec2(34.0, 10.0));
        p.rect_filled(r, 7.0, theme.elevated);
        p.rect_stroke(r, 7.0, egui::Stroke::new(1.0_f32, theme.accent.gamma_multiply(0.8)));
        let icon = if is_image(path) { crate::icons::Icon::Image } else if path.is_dir() { crate::icons::Icon::Folder } else { crate::icons::Icon::Files };
        crate::icons::draw(&p, icon, crate::icons::center_rect(pos2(r.min.x + 13.0, r.center().y), 14.0), theme.accent);
        p.galley(pos2(r.min.x + 26.0, r.center().y - g.size().y / 2.0), g, theme.text);
    }

    /// Opens the file viewer on `path` (used by the dev screenshot tool).
    pub fn open_preview(&mut self, path: PathBuf) {
        self.viewer_loading = true;
        let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = self.tx.send(Req::Preview(path, title));
    }

    pub fn close_viewer(&mut self) {
        self.viewer_tex = None;
        self.viewer = None;
        self.viewer_loading = false;
    }

    pub fn viewer_active(&self) -> bool {
        self.viewer.is_some() || self.viewer_loading
    }

    /// Path to type into the terminal: relative to the followed folder when possible.
    fn insert_text(base: &Path, abs: &Path) -> String {
        let p = abs.strip_prefix(base).map(|p| p.to_path_buf()).unwrap_or_else(|_| abs.to_path_buf());
        format!("{} ", quote(&p.to_string_lossy()))
    }

    fn path_menu(ui: &mut egui::Ui, base: &Path, abs: &Path, actions: &mut Vec<UiAction>) {
        if ui.button("Insert path into terminal").clicked() {
            actions.push(UiAction::Insert(Self::insert_text(base, abs)));
            ui.close_menu();
        }
        if ui.button("Copy path").clicked() {
            actions.push(UiAction::Copy(abs.to_string_lossy().into_owned()));
            ui.close_menu();
        }
        if ui.button("Open with default app").clicked() {
            actions.push(UiAction::OpenExternal(abs.to_path_buf()));
            ui.close_menu();
        }
    }

    // ------------------------------------------------------------------------------- panel UI

    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme, cfg: &mut crate::config::Config) -> Vec<UiAction> {
        let mut actions = Vec::new();
        if let Some(v) = lock(&self.loaded_view).take() {
            self.viewer_tex = None;
            self.viewer = Some(v);
            self.viewer_loading = false;
        }
        self.drag_chip(ctx, theme);
        self.sync_settings(cfg);
        let snap = self.snapshot();
        if let Some(s) = &snap {
            if self.watched_root.as_ref() != Some(&s.root) {
                // New project: start with a collapsed tree.
                self.watched_root = Some(s.root.clone());
                self.expanded.clear();
            }
        }

        egui::SidePanel::left("s-term-sidebar")
            .resizable(true)
            .default_width(340.0)
            .width_range(240.0..=900.0)
            .show_separator_line(false)
            // No fill: the panel sits on the same backdrop as the title bar and rail, so there are
            // no hard edges where they meet. A soft divider fades in from both ends on its right.
            .frame(egui::Frame::none().inner_margin(egui::Margin { left: 12.0, right: 12.0, top: 10.0, bottom: 8.0 }))
            .show_animated(ctx, self.open, |ui| {
                let edge = ui.max_rect() + egui::Margin { left: 12.0, right: 12.0, top: 10.0, bottom: 8.0 };
                crate::glass::fade_line_v(ui.painter(), edge.max.x - 0.5, edge.min.y + 8.0, edge.max.y - 8.0, theme.border);
                let (title, count) = match self.tab {
                    Tab::Changes => ("SOURCE CONTROL", snap.as_ref().and_then(|s| s.git.as_ref()).map(|g| g.changes.len())),
                    Tab::Files => ("EXPLORER", None),
                    Tab::Ai => ("AI USAGE", None),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).size(11.0).strong().color(theme.muted));
                    if let Some(n) = count.filter(|&n| n > 0) {
                        ui.label(RichText::new(format!(" {n} ")).size(10.5).color(theme.bar).background_color(theme.accent));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::icons::button(ui, crate::icons::Icon::Close, 22.0, theme.muted, theme.header, false, Id::new("s-term-sb-close"))
                            .on_hover_text("Hide panel (Ctrl+Shift+B)")
                            .clicked()
                        {
                            self.open = false;
                        }
                        if self.tab == Tab::Files
                            && crate::icons::button(ui, crate::icons::Icon::Plus, 22.0, theme.muted, theme.header, self.adding.is_some(), Id::new("s-term-sb-add"))
                                .on_hover_text("Add a folder to the Explorer")
                                .clicked()
                        {
                            self.adding = if self.adding.is_some() { None } else { Some(String::new()) };
                        }
                    });
                });
                ui.add_space(8.0);

                if self.tab == Tab::Ai {
                    let usage = self.usage.snapshot();
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        crate::usage_ui::tab(ui, theme, usage.as_deref(), &mut self.usage_ui, cfg);
                    });
                    return;
                }
                if self.tab == Tab::Files {
                    self.files(ui, theme, snap.as_deref(), cfg, &mut actions);
                    return;
                }
                let Some(snap) = snap else {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new("Loading…").color(theme.muted)));
                    return;
                };
                self.header(ui, theme, &snap);
                ui.add_space(10.0);
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.changes(ui, theme, &snap, &mut actions));
            });
        // After the rows had their chance to report the drop: clear a drag whose row vanished
        // (panel closed, folder collapsed) so the chip doesn't stick to the pointer.
        if self.dragging.is_some() && !ctx.input(|i| i.pointer.any_down()) {
            self.dragging = None;
        }
        actions
    }

    /// Project card: folder icon, name, branch chip, path.
    fn header(&self, ui: &mut egui::Ui, theme: &Theme, snap: &Snapshot) {
        let name = snap.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
        let home = std::env::var("HOME").unwrap_or_default();
        let mut shown = snap.dir.to_string_lossy().into_owned();
        if !home.is_empty() && shown.starts_with(&home) {
            shown = format!("~{}", &shown[home.len()..]);
        }
        crate::usage_ui::card(ui, theme, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                ui.painter().rect_filled(r, 8.0, theme.header_focused);
                crate::icons::draw(ui.painter(), crate::icons::Icon::Folder, crate::icons::center_rect(r.center(), 16.0), theme.accent);
                ui.vertical(|ui| {
                    ui.label(RichText::new(name).size(14.0).strong().color(theme.text));
                    ui.label(RichText::new(shown).monospace().size(10.5).color(theme.muted));
                });
            });
            if let Some(g) = &snap.git {
                ui.add_space(6.0);
                // Right side (change count, ahead/behind) is placed first; the branch gets the space that's
                // left and ends in "…" when it doesn't fit, so a long branch name never runs into it.
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
                let p = ui.painter();
                let cy = r.center().y;
                let n = g.changes.len();
                let (txt, col) = if n == 0 { ("clean".to_string(), c32(theme.palette[2])) } else { (format!("{n} changed"), c32(theme.palette[3])) };
                let rg = p.layout_no_wrap(txt, FontId::proportional(11.0), col);
                let mut x = r.max.x - rg.size().x;
                p.galley(pos2(x, cy - rg.size().y / 2.0), rg, col);
                let mut ab = Vec::new();
                if g.ahead > 0 {
                    ab.push(format!("↑{}", g.ahead));
                }
                if g.behind > 0 {
                    ab.push(format!("↓{}", g.behind));
                }
                if !ab.is_empty() {
                    let ag = p.layout_no_wrap(ab.join(" "), FontId::monospace(11.5), theme.muted);
                    x -= ag.size().x + 8.0;
                    p.galley(pos2(x, cy - ag.size().y / 2.0), ag, theme.muted);
                }
                crate::icons::draw(p, crate::icons::Icon::Git, crate::icons::center_rect(pos2(r.min.x + 7.0, cy), 14.0), theme.accent2);
                let mut job = egui::text::LayoutJob::simple_singleline(g.branch.clone(), FontId::monospace(12.0), theme.accent2);
                job.wrap = egui::text::TextWrapping { max_width: (x - 12.0 - (r.min.x + 20.0)).max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let bg = p.layout_job(job);
                p.galley(pos2(r.min.x + 20.0, cy - bg.size().y / 2.0), bg, theme.accent2);
                resp.on_hover_text(&g.branch);
            }
        });
    }

    fn section(ui: &mut egui::Ui, theme: &Theme, text: &str) {
        ui.add_space(8.0);
        ui.label(RichText::new(text).monospace().size(10.5).color(theme.muted));
        ui.add_space(2.0);
    }

    /// One list row; returns its response. `paint` draws the content inside the row rect.
    fn row(ui: &mut egui::Ui, theme: &Theme, id: Id, paint: impl FnOnce(&egui::Painter, Rect)) -> egui::Response {
        // Stable id (not a positional one) so a drag survives the tree refreshing under it.
        let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW_H));
        let resp = ui.interact(rect, id.with("row"), Sense::click_and_drag());
        let hover = ui.ctx().animate_bool_with_time(id, resp.hovered(), 0.1);
        if hover > 0.0 {
            ui.painter().rect_filled(rect, 5.0, lerp_color(Color32::TRANSPARENT, theme.header, hover));
        }
        paint(&ui.painter().with_clip_rect(rect), rect);
        resp
    }

    fn changes(&mut self, ui: &mut egui::Ui, theme: &Theme, snap: &Snapshot, actions: &mut Vec<UiAction>) {
        let Some(g) = &snap.git else {
            ui.add_space(12.0);
            ui.label(RichText::new("Not a git repository.\nThe Files tab still works.").color(theme.muted));
            return;
        };
        if g.changes.is_empty() {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("✓ Working tree clean").color(c32(theme.palette[2]))));
        }
        let font = FontId::monospace(12.5);
        let small = FontId::monospace(11.0);
        let mut open = None;
        for c in &g.changes {
            let (when, age) = ago(c.mtime);
            let recent = (1.0 - age / RECENT_SECS).clamp(0.0, 1.0);
            let (dir, name) = match c.rel.rsplit_once('/') {
                Some((d, n)) => (format!("{d}/"), n.to_string()),
                None => (String::new(), c.rel.clone()),
            };
            let resp = Self::row(ui, theme, Id::new(("s-term-ch", &c.rel)), |p, r| {
                let kc = c.kind.color(theme);
                let badge = Rect::from_min_size(pos2(r.min.x + 6.0, r.center().y - 8.0), vec2(16.0, 16.0));
                p.rect_filled(badge, 4.0, kc.gamma_multiply(0.18));
                p.text(badge.center(), Align2::CENTER_CENTER, c.kind.badge(), small.clone(), kc);

                // Right side: +adds −dels and age.
                let mut x = r.max.x - 6.0;
                let rr = p.text(pos2(x, r.center().y), Align2::RIGHT_CENTER, &when, small.clone(), lerp_color(theme.muted, theme.accent, recent));
                x = rr.min.x - 8.0;
                if let Some(d) = c.dels.filter(|&d| d > 0) {
                    let rr = p.text(pos2(x, r.center().y), Align2::RIGHT_CENTER, format!("−{d}"), small.clone(), c32(theme.palette[1]));
                    x = rr.min.x - 4.0;
                }
                if let Some(a) = c.adds.filter(|&a| a > 0) {
                    let rr = p.text(pos2(x, r.center().y), Align2::RIGHT_CENTER, format!("+{a}"), small.clone(), c32(theme.palette[2]));
                    x = rr.min.x - 4.0;
                }

                let text_clip = p.with_clip_rect(Rect::from_min_max(r.min, pos2(x - 4.0, r.max.y)));
                let nr = text_clip.text(pos2(badge.max.x + 8.0, r.center().y), Align2::LEFT_CENTER, &name, font.clone(), Color32::WHITE);
                text_clip.text(pos2(nr.max.x + 6.0, r.center().y), Align2::LEFT_CENTER, &dir, small.clone(), theme.muted);
                if recent > 0.0 {
                    p.circle_filled(pos2(r.min.x + 2.0, r.center().y), 2.5, theme.accent.gamma_multiply(recent));
                }
            });
            let resp = resp.on_hover_text(&c.rel);
            if resp.clicked() {
                open = Some(c);
            }
            if resp.middle_clicked() {
                actions.push(UiAction::Insert(Self::insert_text(&snap.dir, &snap.root.join(&c.rel))));
            }
            Self::drag_source(&mut self.dragging, &resp, &snap.root.join(&c.rel), actions);
            resp.context_menu(|ui| Self::path_menu(ui, &snap.dir, &snap.root.join(&c.rel), actions));
        }
        if let Some(c) = open {
            self.open_diff(&snap.root, c);
        }
        if g.truncated {
            ui.label(RichText::new(format!("… showing first {MAX_CHANGES}")).color(theme.muted));
        }

        if !g.commits.is_empty() {
            Self::section(ui, theme, "RECENT COMMITS");
            for c in &g.commits {
                Self::commit_row(ui, theme, c, actions);
            }
        }
    }

    /// Two-line commit row: avatar, subject, then author · hash · age, with an AI badge.
    fn commit_row(ui: &mut egui::Ui, theme: &Theme, c: &Commit, actions: &mut Vec<UiAction>) {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), CLICK);
        let hover = ui.ctx().animate_bool_with_time(Id::new(("s-term-commit", &c.hash)), resp.hovered(), 0.1);
        if hover > 0.0 {
            ui.painter().rect_filled(r, 5.0, lerp_color(Color32::TRANSPARENT, theme.header, hover));
        }
        let p = ui.painter().with_clip_rect(r);
        let small = FontId::monospace(11.0);
        let col = author_color(&c.author);

        let ac = pos2(r.min.x + 16.0, r.center().y);
        p.circle_filled(ac, 11.0, col.gamma_multiply(0.25));
        p.circle_stroke(ac, 11.0, egui::Stroke::new(1.0_f32, col.gamma_multiply(0.7)));
        p.text(ac, Align2::CENTER_CENTER, initials(&c.author), FontId::proportional(10.5), col);
        if let Some(agent) = c.agent() {
            // Badge on the avatar's corner.
            let bc = ac + vec2(8.5, 8.0);
            p.circle_filled(bc, 6.0, theme.bar);
            match agent {
                Agent::Claude => crate::usage_ui::claude_icon(&p, bc, 4.6),
                Agent::Gemini => crate::usage_ui::gemini_icon(&p, bc, 5.0),
            }
        }

        let x = r.min.x + 34.0;
        let top = r.min.y + 11.0;
        let bottom = r.min.y + 27.0;
        let wr = p.text(pos2(r.max.x - 6.0, top), Align2::RIGHT_CENTER, short_age(&c.when), small.clone(), theme.muted);
        p.with_clip_rect(Rect::from_min_max(r.min, pos2(wr.min.x - 6.0, r.max.y)))
            .text(pos2(x, top), Align2::LEFT_CENTER, &c.subject, FontId::monospace(12.0), Color32::from_gray(225));
        let ar = p.text(pos2(x, bottom), Align2::LEFT_CENTER, &c.author, small.clone(), col);
        let mut tail = format!("  {}", c.hash);
        if !c.coauthors.is_empty() {
            let names: Vec<&str> = c.coauthors.iter().map(|a| a.split(" <").next().unwrap_or(a)).collect();
            tail = format!(" + {}{tail}", names.join(", "));
        }
        p.text(pos2(ar.max.x, bottom), Align2::LEFT_CENTER, tail, small, theme.muted);

        let resp = resp.on_hover_ui(|ui| {
            ui.label(RichText::new(&c.subject).strong());
            ui.label(RichText::new(format!("{} <{}>", c.author, c.email)).color(col));
            for a in &c.coauthors {
                ui.label(RichText::new(format!("Co-authored-by: {a}")).color(theme.muted));
            }
            ui.label(RichText::new(format!("{} · {} ({})", c.hash, c.date, c.when)).monospace().color(theme.muted));
            ui.label(RichText::new("Middle-click: insert hash · right-click: copy").size(10.5).color(theme.muted));
        });
        if resp.middle_clicked() {
            actions.push(UiAction::Insert(format!("{} ", c.hash)));
        }
        resp.context_menu(|ui| {
            if ui.button("Insert hash into terminal").clicked() {
                actions.push(UiAction::Insert(format!("{} ", c.hash)));
                ui.close_menu();
            }
            if ui.button("Copy hash").clicked() {
                actions.push(UiAction::Copy(c.hash.clone()));
                ui.close_menu();
            }
            if ui.button("Copy author").clicked() {
                actions.push(UiAction::Copy(format!("{} <{}>", c.author, c.email)));
                ui.close_menu();
            }
        });
    }

    fn tree(ui: &mut egui::Ui, cx: &mut TreeCx<'_>, dir: &Path, depth: usize) {
        let (theme, snap) = (cx.theme, cx.snap);
        let Some(entries) = snap.tree.get(dir) else {
            if depth > 0 {
                ui.label(RichText::new("    …").color(theme.muted));
            }
            return;
        };
        let font = FontId::monospace(12.5);
        let small = FontId::monospace(11.0);
        for e in entries {
            let is_open = e.is_dir && cx.expanded.contains(&e.path);
            let git = snap.git.as_ref();
            let ignored = git.is_some_and(|g| g.ignored.contains(&e.path));
            if ignored && cx.hide_ignored {
                continue;
            }
            let kind = git.and_then(|g| g.by_path.get(&e.path).copied());
            // A folder is "dirty" if anything inside it changed.
            let dirty_dir = e.is_dir && git.is_some_and(|g| g.by_path.keys().any(|p| p.starts_with(&e.path)));
            let (when, age) = ago(e.mtime);
            let recent = if e.is_dir { 0.0 } else { (1.0 - age / RECENT_SECS).clamp(0.0, 1.0) };

            let resp = Self::row(ui, theme, Id::new(("s-term-tree", &e.path)), |p, r| {
                let x = r.min.x + 8.0 + depth as f32 * 14.0;
                let y = r.center().y;
                let mut color = if e.is_dir { theme.accent2 } else { Color32::from_gray(215) };
                if let Some(k) = kind {
                    color = k.color(theme);
                } else if dirty_dir {
                    color = lerp_color(color, c32(theme.palette[3]), 0.6);
                }
                if ignored {
                    color = theme.muted.gamma_multiply(0.7);
                }
                if e.is_dir {
                    p.text(pos2(x, y), Align2::LEFT_CENTER, if is_open { "▾" } else { "▸" }, small.clone(), theme.muted);
                } else if is_image(&e.path) {
                    let ic = if ignored { color } else { theme.accent2.gamma_multiply(0.9) };
                    crate::icons::draw(p, crate::icons::Icon::Image, crate::icons::center_rect(pos2(x + 4.0, y), 12.0), ic);
                }
                let nr = p.text(pos2(x + 14.0, y), Align2::LEFT_CENTER, &e.name, font.clone(), color);
                if let Some(k) = kind {
                    p.text(pos2(nr.max.x + 6.0, y), Align2::LEFT_CENTER, k.badge(), small.clone(), k.color(theme));
                }
                if recent > 0.0 {
                    p.text(pos2(r.max.x - 6.0, y), Align2::RIGHT_CENTER, &when, small.clone(), theme.accent.gamma_multiply(recent.max(0.4)));
                    p.circle_filled(pos2(r.min.x + 2.0, y), 2.5, theme.accent.gamma_multiply(recent));
                }
            });
            if resp.clicked() {
                if e.is_dir {
                    cx.toggled = Some(e.path.clone());
                } else {
                    let title = e.path.strip_prefix(&snap.root).unwrap_or(&e.path).to_string_lossy().into_owned();
                    cx.preview = Some((e.path.clone(), title));
                }
            }
            if resp.middle_clicked() {
                cx.actions.push(UiAction::Insert(Self::insert_text(cx.base, &e.path)));
            }
            Self::drag_source(cx.dragging, &resp, &e.path, cx.actions);
            let resp = if resp.dragged() { resp } else { resp.on_hover_text("Drag into a pane to insert the path") };
            resp.context_menu(|ui| {
                Self::path_menu(ui, cx.base, &e.path, cx.actions);
                if e.is_dir && ui.button("Add to Explorer as a section").clicked() {
                    cx.pin = Some(e.path.clone());
                    ui.close_menu();
                }
            });
            if is_open {
                Self::tree(ui, cx, &e.path, depth + 1);
            }
        }
    }

    /// The Files tab: the followed project on top, then one section per extra folder,
    /// stacked and sharing the height.
    fn files(&mut self, ui: &mut egui::Ui, theme: &Theme, snap: Option<&Snapshot>, cfg: &mut crate::config::Config, actions: &mut Vec<UiAction>) {
        if let Some(p) = lock(&self.picked).take() {
            Self::add_folder(cfg, p);
            self.adding = None;
        }
        if self.adding.is_some() {
            self.add_row(ui, theme, cfg);
            ui.add_space(8.0);
        }
        let base = snap.map(|s| s.dir.clone()).unwrap_or_default();
        let folders = cfg.explorer_folders.clone();
        let extras = lock(&self.extra_snaps).clone();
        let gap = 8.0;
        let width = ui.available_width();
        let mut preview = None;
        let mut pin = None;

        // ---- followed project (its card sits above the shared area, so every tree gets the same height)
        if let Some(snap) = snap {
            self.header(ui, theme, snap);
            ui.add_space(10.0);
        }
        let heads = folders.len() as f32 * (EXTRA_HEAD_H + gap);
        let main_key = PathBuf::new();
        let open_keys: Vec<PathBuf> =
            std::iter::once(main_key.clone()).chain(folders.iter().filter(|f| !self.collapsed.contains(*f)).cloned()).collect();
        let trees_h = (ui.available_height() - heads).max(PANE_MIN_H * open_keys.len() as f32);
        let total_w: f32 = open_keys.iter().map(|k| self.pane_weight(k)).sum();
        let height_of = |w: f32| (trees_h * w / total_w).max(PANE_MIN_H);
        let each = height_of(self.pane_weight(&main_key));
        ui.allocate_ui(vec2(width, each), |ui| {
            let Some(snap) = snap else {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.label(RichText::new("Loading…").color(theme.muted)));
                ui.allocate_space(ui.available_size());
                return;
            };
            egui::ScrollArea::vertical().id_salt("s-term-files-main").auto_shrink(false).show(ui, |ui| {
                let mut cx = TreeCx::new(theme, snap, &base, &self.expanded, cfg.hide_ignored, actions, &mut self.dragging);
                Self::tree(ui, &mut cx, &snap.root, 0);
                (preview, pin) = (cx.preview.take().or(preview.take()), cx.pin.take());
                if let Some(d) = cx.toggled.take() {
                    if !self.expanded.remove(&d) {
                        self.expanded.insert(d);
                    }
                    self.send_watch();
                }
            });
        });

        // ---- extra folders
        let mut remove = None;
        let mut above = main_key;
        for (i, folder) in folders.iter().enumerate() {
            // Splitter between the open pane above and the next open pane below.
            let below = folders[i..].iter().find(|f| !self.collapsed.contains(*f)).cloned();
            let (delta, reset) = Self::pane_splitter(ui, theme, gap, ("s-term-files-split", folder), below.is_some());
            if reset {
                self.pane_weights.clear();
            } else if let (Some(below), true) = (&below, delta != 0.0) {
                let (a, b) = (self.pane_weight(&above), self.pane_weight(below));
                let min_w = PANE_MIN_H * total_w / trees_h;
                if a + b > 2.0 * min_w {
                    let dw = (delta * total_w / trees_h).clamp(min_w - a, b - min_w);
                    self.pane_weights.insert(above.clone(), a + dw);
                    self.pane_weights.insert(below.clone(), b - dw);
                }
            }
            let collapsed = self.collapsed.contains(folder);
            if !collapsed {
                above = folder.clone();
            }
            let fsnap = extras.iter().find(|s| &s.root == folder);
            let head = self.folder_head(ui, theme, folder, fsnap.map(|s| &**s), collapsed);
            if head.toggle {
                if !self.collapsed.remove(folder) {
                    self.collapsed.insert(folder.clone());
                }
            }
            if head.remove {
                remove = Some(folder.clone());
            }
            if let Some(a) = head.action {
                actions.push(a);
            }
            if collapsed {
                continue;
            }
            let each = height_of(self.pane_weight(folder));
            ui.allocate_ui(vec2(width, each), |ui| {
                let Some(fsnap) = fsnap else {
                    let msg = if folder.is_dir() { "Loading…" } else { "Folder not found" };
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new(msg).color(theme.muted)));
                    ui.allocate_space(ui.available_size());
                    return;
                };
                egui::ScrollArea::vertical().id_salt(("s-term-files", folder)).auto_shrink(false).show(ui, |ui| {
                    let mut cx = TreeCx::new(theme, fsnap, &base, &self.extra_expanded, cfg.hide_ignored, actions, &mut self.dragging);
                    Self::tree(ui, &mut cx, &fsnap.root, 0);
                    if let Some(p) = cx.preview.take() {
                        preview = Some(p);
                    }
                    if let Some(p) = cx.pin.take() {
                        pin = Some(p);
                    }
                    if let Some(d) = cx.toggled.take() {
                        if !self.extra_expanded.remove(&d) {
                            self.extra_expanded.insert(d);
                        }
                        self.send_watch();
                    }
                });
            });
        }
        if let Some(f) = remove {
            cfg.explorer_folders.retain(|x| *x != f);
            self.collapsed.remove(&f);
            self.pane_weights.remove(&f);
        }
        if let Some(p) = pin {
            Self::add_folder(cfg, p);
        }
        if let Some((p, t)) = preview {
            self.viewer_loading = true;
            let _ = self.tx.send(Req::Preview(p, t));
        }
    }

    fn pane_weight(&self, key: &Path) -> f32 {
        self.pane_weights.get(key).copied().unwrap_or(1.0)
    }

    /// A hairline between two Explorer panes; drag it to resize them, double-click to even them out.
    /// Returns the vertical drag delta and whether a reset was asked for.
    fn pane_splitter(ui: &mut egui::Ui, theme: &Theme, h: f32, id: impl std::hash::Hash, active: bool) -> (f32, bool) {
        let sense = if active { Sense::click_and_drag() } else { Sense::hover() };
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        // A slightly taller grab zone than the gap itself, so the line is easy to hit.
        let resp = ui.interact(r.expand2(vec2(0.0, 2.0)), Id::new(id), sense);
        let hot = active && (resp.hovered() || resp.dragged());
        let t = ui.ctx().animate_bool_with_time(resp.id, hot, 0.12);
        if hot {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        let y = r.center().y;
        let inset = 10.0 * (1.0 - t);
        let color = lerp_color(theme.border.gamma_multiply(0.55), theme.accent.gamma_multiply(0.8), t);
        ui.painter().hline(r.min.x + inset..=r.max.x - inset, y, egui::Stroke::new(1.0 + t, color));
        if !active {
            return (0.0, false);
        }
        let resp = resp.on_hover_text("Drag to resize · double-click to reset");
        (resp.drag_delta().y, resp.double_clicked())
    }

    /// Adds `p` to the Explorer's extra folders (no duplicates, `~` expanded).
    pub fn add_folder(cfg: &mut crate::config::Config, p: PathBuf) {
        let p = match (p.strip_prefix("~"), std::env::var_os("HOME")) {
            (Ok(rest), Some(h)) => PathBuf::from(h).join(rest),
            _ => p,
        };
        let p = std::fs::canonicalize(&p).unwrap_or(p);
        if !cfg.explorer_folders.contains(&p) {
            cfg.explorer_folders.push(p);
        }
    }

    /// Inline "add folder" field: type a path, or browse.
    fn add_row(&mut self, ui: &mut egui::Ui, theme: &Theme, cfg: &mut crate::config::Config) {
        let Some(text) = self.adding.as_mut() else { return };
        let mut done = false;
        let mut browse = false;
        crate::usage_ui::card(ui, theme, |ui| {
            ui.label(RichText::new("Add a folder to the Explorer").size(12.0).color(theme.text));
            ui.add_space(4.0);
            let r = ui.add(egui::TextEdit::singleline(text).hint_text("~/projects/other").desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
            if !r.has_focus() && !r.lost_focus() && text.is_empty() {
                r.request_focus();
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let path = PathBuf::from(text.trim());
            let expanded = match (path.strip_prefix("~"), std::env::var_os("HOME")) {
                (Ok(rest), Some(h)) => PathBuf::from(h).join(rest),
                _ => path.clone(),
            };
            let valid = !text.trim().is_empty() && expanded.is_dir();
            if !text.trim().is_empty() && !valid {
                ui.label(RichText::new("Not a folder").size(10.5).color(c32(theme.palette[1])));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(valid, egui::Button::new("Add")).clicked() || (enter && valid) {
                    Self::add_folder(cfg, path.clone());
                    done = true;
                }
                if ui.button("Browse…").clicked() {
                    browse = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    done = true;
                }
            });
        });
        if browse {
            self.browse();
        }
        if done {
            self.adding = None;
        }
    }

    /// Header of an extra Explorer section: fold arrow, folder name, path, branch and a remove button.
    fn folder_head(&self, ui: &mut egui::Ui, theme: &Theme, folder: &Path, snap: Option<&Snapshot>, collapsed: bool) -> FolderHead {
        let mut out = FolderHead::default();
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), EXTRA_HEAD_H), Sense::click());
        let hover = ui.ctx().animate_bool_with_time(Id::new(("s-term-xhead", folder)), resp.hovered(), 0.1);
        let p = ui.painter().with_clip_rect(r);
        p.rect_filled(r, 8.0, lerp_color(theme.header.gamma_multiply(0.5), theme.header, hover));
        let y = r.center().y;
        let chev = if collapsed { crate::icons::Icon::ChevronRight } else { crate::icons::Icon::ChevronDown };
        crate::icons::draw(&p, chev, crate::icons::center_rect(pos2(r.min.x + 12.0, y), 11.0), theme.muted);
        crate::icons::draw(&p, crate::icons::Icon::Folder, crate::icons::center_rect(pos2(r.min.x + 30.0, y), 14.0), theme.accent);
        let close = Rect::from_center_size(pos2(r.max.x - 14.0, y), vec2(20.0, 20.0));
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| folder.to_string_lossy().into_owned());
        let nr = p.text(pos2(r.min.x + 44.0, y), Align2::LEFT_CENTER, &name, FontId::proportional(13.0), theme.text);
        let tail = match snap.and_then(|s| s.git.as_ref()) {
            Some(g) if !g.changes.is_empty() => format!("{} · {} changed", g.branch, g.changes.len()),
            Some(g) => g.branch.clone(),
            None => String::new(),
        };
        p.with_clip_rect(Rect::from_min_max(r.min, pos2(close.min.x - 4.0, r.max.y)))
            .text(pos2(nr.max.x + 8.0, y), Align2::LEFT_CENTER, tail, FontId::monospace(10.5), theme.accent2.gamma_multiply(0.85));
        let x = ui.interact(close, Id::new(("s-term-xhead-rm", folder)), CLICK);
        let xc = if x.hovered() { c32(theme.palette[1]) } else { theme.muted.gamma_multiply(0.4 + 0.6 * hover) };
        crate::icons::draw(&p, crate::icons::Icon::Close, crate::icons::center_rect(close.center(), 12.0), xc);
        if x.on_hover_text("Remove from Explorer").clicked() {
            out.remove = true;
        } else if resp.clicked() {
            out.toggle = true;
        }
        let resp = resp.on_hover_text(folder.to_string_lossy());
        resp.context_menu(|ui| {
            if ui.button("Copy path").clicked() {
                out.action = Some(UiAction::Copy(folder.to_string_lossy().into_owned()));
                ui.close_menu();
            }
            if ui.button("Open with default app").clicked() {
                out.action = Some(UiAction::OpenExternal(folder.to_path_buf()));
                ui.close_menu();
            }
            if ui.button("Remove from Explorer").clicked() {
                out.remove = true;
                ui.close_menu();
            }
        });
        out
    }

    // ------------------------------------------------------------------------------ viewer UI

    /// Diff / file viewer floating over the terminal area.
    pub fn show_viewer(&mut self, ctx: &egui::Context, theme: &Theme, area: Rect) -> Vec<UiAction> {
        let mut actions = Vec::new();
        if let Some(v) = lock(&self.loaded_view).take() {
            self.viewer_tex = None;
            self.viewer = Some(v);
            self.viewer_loading = false;
        }
        if !self.viewer_active() {
            return actions;
        }
        let mut close = false;

        egui::Area::new(Id::new("s-term-viewer-backdrop"))
            .order(egui::Order::Middle)
            .fixed_pos(area.min)
            .show(ctx, |ui| {
                let (r, resp) = ui.allocate_exact_size(area.size(), CLICK);
                ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(140));
                if resp.clicked() {
                    close = true;
                }
            });

        let rect = area.shrink2(vec2((area.width() * 0.05).max(16.0), (area.height() * 0.05).max(16.0)));
        egui::Area::new(Id::new("s-term-viewer"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(c32(theme.bg))
                    .rounding(18.0)
                    .stroke(egui::Stroke::new(1.0_f32, theme.rim))
                    .shadow(egui::epaint::Shadow { offset: vec2(0.0, 8.0), blur: 30.0, spread: 0.0, color: Color32::from_black_alpha(160) })
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        let inner = rect.size() - vec2(24.0, 24.0);
                        ui.set_min_size(inner);
                        ui.set_max_size(inner);
                        let Some(v) = &self.viewer else {
                            ui.centered_and_justified(|ui| ui.label(RichText::new("Loading…").color(theme.muted)));
                            return;
                        };
                        ui.horizontal(|ui| {
                            let kind = if v.is_diff { "DIFF" } else if v.image.is_some() { "IMAGE" } else { "FILE" };
                            ui.label(RichText::new(format!(" {kind} ")).monospace().size(11.0).color(theme.bar).background_color(theme.accent));
                            ui.label(RichText::new(&v.title).monospace().strong().size(13.5).color(Color32::WHITE));
                            if v.is_diff {
                                ui.label(RichText::new(format!("+{}", v.adds)).monospace().color(c32(theme.palette[2])));
                                ui.label(RichText::new(format!("−{}", v.dels)).monospace().color(c32(theme.palette[1])));
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("✕  Esc").clicked() {
                                    close = true;
                                }
                                if ui.button("Open").on_hover_text("Open with the default app").clicked() {
                                    actions.push(UiAction::OpenExternal(v.abs.clone()));
                                }
                                if ui.button("Copy path").clicked() {
                                    actions.push(UiAction::Copy(v.abs.to_string_lossy().into_owned()));
                                }
                                if ui.button("Insert path").on_hover_text("Type the path into the focused terminal").clicked() {
                                    if let Some(s) = lock(&self.snap).clone() {
                                        actions.push(UiAction::Insert(Self::insert_text(&s.dir, &v.abs)));
                                    }
                                    close = true;
                                }
                            });
                        });
                        ui.add_space(6.0);
                        if let Some(n) = &v.note {
                            ui.label(RichText::new(n).color(theme.muted));
                        }
                        if let Some(img) = &v.image {
                            let tex = self.viewer_tex.get_or_insert_with(|| {
                                ui.ctx().load_texture("s-term-viewer-image", img.clone(), egui::TextureOptions::LINEAR)
                            });
                            Self::image(ui, theme, tex);
                        } else {
                            Self::lines(ui, theme, v);
                        }
                    });
            });

        if close {
            self.close_viewer();
        }
        actions
    }

    /// Image fitted into the remaining space (never upscaled past 2×), on a checkerboard.
    fn image(ui: &mut egui::Ui, theme: &Theme, tex: &egui::TextureHandle) {
        let avail = ui.available_rect_before_wrap();
        ui.allocate_rect(avail, Sense::hover());
        let size = tex.size_vec2() / ui.ctx().pixels_per_point();
        let scale = (avail.width() / size.x).min(avail.height() / size.y).min(2.0);
        let r = Rect::from_center_size(avail.center(), size * scale);
        let p = ui.painter().with_clip_rect(avail);
        // Checkerboard so transparency is visible.
        let cell = 10.0;
        p.rect_filled(r, 0.0, Color32::from_gray(38));
        let (nx, ny) = ((r.width() / cell).ceil() as i32, (r.height() / cell).ceil() as i32);
        if nx * ny < 40_000 {
            for j in 0..ny {
                for i in (j % 2..nx).step_by(2) {
                    let c = Rect::from_min_size(r.min + vec2(i as f32 * cell, j as f32 * cell), vec2(cell, cell)).intersect(r);
                    p.rect_filled(c, 0.0, Color32::from_gray(48));
                }
            }
        }
        p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        p.rect_stroke(r, 0.0, egui::Stroke::new(1.0_f32, theme.rim));
    }

    fn lines(ui: &mut egui::Ui, theme: &Theme, v: &FileView) {
        let font = FontId::monospace(13.0);
        let (cw, row_h) = ui.fonts(|f| (f.glyph_width(&font, 'M'), f.row_height(&font) + 2.0));
        let gutter = if v.is_diff { 0.0 } else { cw * (v.lines.len().max(1).ilog10() as f32 + 2.0) + 8.0 };
        let content_w = (gutter + v.max_len as f32 * cw + 24.0).max(ui.available_width());
        let green = c32(theme.palette[2]);
        let red = c32(theme.palette[1]);
        egui::ScrollArea::both().auto_shrink(false).show_rows(ui, row_h, v.lines.len(), |ui, range| {
            for i in range {
                let (kind, text) = &v.lines[i];
                let (r, _) = ui.allocate_exact_size(vec2(content_w, row_h), Sense::hover());
                let p = ui.painter();
                let (bg, fg) = match kind {
                    LineKind::Add => (Some(green.gamma_multiply(0.12)), lerp_color(green, Color32::WHITE, 0.3)),
                    LineKind::Del => (Some(red.gamma_multiply(0.12)), lerp_color(red, Color32::WHITE, 0.3)),
                    LineKind::Hunk => (Some(theme.accent2.gamma_multiply(0.08)), theme.accent2),
                    LineKind::Meta => (None, theme.muted),
                    LineKind::Context => (None, Color32::from_gray(205)),
                };
                if let Some(bg) = bg {
                    p.rect_filled(r, 0.0, bg);
                }
                if !v.is_diff {
                    p.text(pos2(r.min.x + gutter - 12.0, r.center().y), Align2::RIGHT_CENTER, (i + 1).to_string(), font.clone(), theme.muted.gamma_multiply(0.7));
                }
                p.text(pos2(r.min.x + gutter, r.center().y), Align2::LEFT_CENTER, text, font.clone(), fg);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_header() {
        assert_eq!(parse_branch("main...origin/main [ahead 2, behind 1]"), ("main".into(), 2, 1));
        assert_eq!(parse_branch("No commits yet on main"), ("main".into(), 0, 0));
        assert_eq!(parse_branch("feature"), ("feature".into(), 0, 0));
    }

    #[test]
    fn change_kinds() {
        assert!(ChangeKind::from_xy('?', '?') == ChangeKind::Untracked);
        assert!(ChangeKind::from_xy(' ', 'M') == ChangeKind::Modified);
        assert!(ChangeKind::from_xy('A', ' ') == ChangeKind::Added);
        assert!(ChangeKind::from_xy('U', 'U') == ChangeKind::Conflict);
    }

    #[test]
    fn real_repo_snapshot() {
        let dir = std::env::temp_dir().join(format!("s-term-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let sh = |args: &[&str]| {
            assert!(Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success(), "{args:?}");
        };
        sh(&["init", "-q", "-b", "main"]);
        sh(&["config", "user.email", "t@t"]);
        sh(&["config", "user.name", "t"]);
        std::fs::write(dir.join("src/a.rs"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "first commit"]);
        std::fs::write(dir.join("src/a.rs"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(dir.join("new file.txt"), "x\ny\n").unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();

        let snap = build_snapshot(&dir.join("src"), &[], true);
        assert_eq!(snap.root.canonicalize().unwrap(), dir.canonicalize().unwrap());
        let g = snap.git.as_ref().unwrap();
        assert_eq!(g.branch, "main");
        let a = g.changes.iter().find(|c| c.rel == "src/a.rs").unwrap();
        assert!(a.kind == ChangeKind::Modified && a.adds == Some(2) && a.dels == Some(1));
        let n = g.changes.iter().find(|c| c.rel == "new file.txt").unwrap();
        assert!(n.kind == ChangeKind::Untracked && n.adds == Some(2));
        assert_eq!(g.commits[0].subject, "first commit");
        assert!(g.ignored.iter().any(|p| p.ends_with("target")));
        assert!(snap.tree[&snap.root].iter().any(|e| e.name == "src" && e.is_dir));

        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        sh(&["add", "b.txt"]);
        sh(&["commit", "-q", "-m", "add b", "-m", "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"]);
        let g2 = git_info(&dir).unwrap();
        assert_eq!(g2.commits[0].subject, "add b");
        assert_eq!(g2.commits[0].author, "t");
        assert_eq!(g2.commits[0].coauthors, vec!["Claude Opus 5.5 <noreply@anthropic.com>".to_string()]);
        assert!(g2.commits[0].agent() == Some(Agent::Claude));
        assert!(g2.commits[1].agent().is_none());

        let d = load_diff(&snap.root, "src/a.rs", false);
        assert_eq!((d.adds, d.dels), (2, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_parsing() {
        let raw = "abc123\x1fAdd panel\x1f2 hours ago\x1f2026-11-24 08:00\x1fSeb Paroc\x1fseb@x\x1fClaude Opus 5.5 <noreply@anthropic.com>";
        let c = parse_commit(raw).unwrap();
        assert_eq!(c.author, "Seb Paroc");
        assert_eq!(c.coauthors, vec!["Claude Opus 5.5 <noreply@anthropic.com>".to_string()]);
        assert!(c.agent() == Some(Agent::Claude));
        assert_eq!(initials("Seb Paroc"), "SP");
        assert_eq!(short_age("46 minutes ago"), "46m");
        assert_eq!(short_age("2 years, 3 months ago"), "2y");
        let plain = parse_commit("a\x1fs\x1fw\x1fd\x1fAna\x1fa@b\x1f").unwrap();
        assert!(plain.agent().is_none() && plain.coauthors.is_empty());
    }

    #[test]
    fn image_preview() {
        let path = std::env::temp_dir().join(format!("s-term-img-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 0, 0, 128])).save(&path).unwrap();
        assert!(is_image(&path) && !is_image(Path::new("a.rs")));
        let v = load_preview(&path, "x".into());
        let img = v.image.expect("decoded");
        assert_eq!(img.size, [3, 2]);
        assert!(v.note.unwrap().starts_with("3×2"));
        let _ = std::fs::remove_file(&path);
    }

    /// Drives a real egui frame loop: press on an Explorer row, drag across the window, release.
    #[test]
    fn drag_file_out_of_explorer() {
        use egui::{Event, PointerButton, RawInput};
        let dir = std::env::temp_dir().join(format!("s-term-dnd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pic.png");
        std::fs::write(&file, b"x").unwrap();

        let ctx = egui::Context::default();
        let theme = Theme::preset("graphite");
        let mut cfg = crate::config::Config::default();
        let mut sb = Sidebar::new(ctx.clone());
        sb.show_view(Tab::Files);
        sb.follow(Some(dir.clone()));
        let t0 = Instant::now();
        while sb.snapshot().is_none() {
            assert!(t0.elapsed() < Duration::from_secs(10), "no snapshot");
            std::thread::sleep(Duration::from_millis(20));
        }

        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0));
        let mut all = Vec::new();
        let mut frame = |events: Vec<Event>, all: &mut Vec<UiAction>| {
            let input = RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let _ = ctx.run(input, |ctx| all.extend(sb.show(ctx, &theme, &mut cfg)));
        };
        for _ in 0..5 {
            frame(vec![], &mut all); // let the side panel open and lay out
        }
        let row = ctx.read_response(Id::new(("s-term-tree", &file)).with("row")).expect("row for pic.png").rect.center();
        let target = pos2(900.0, 400.0);
        let btn = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(vec![Event::PointerMoved(row)], &mut all);
        frame(vec![btn(row, true)], &mut all);
        for i in 1..=10 {
            frame(vec![Event::PointerMoved(row + (target - row) * (i as f32 / 10.0))], &mut all);
        }
        frame(vec![btn(target, false)], &mut all);
        frame(vec![], &mut all);

        let drops: Vec<_> = all.iter().filter_map(|a| match a { UiAction::Drop(p, pos) => Some((p.clone(), *pos)), _ => None }).collect();
        assert_eq!(drops.len(), 1, "expected exactly one drop");
        assert_eq!(drops[0].0, file);
        assert!((drops[0].1 - target).length() < 1.0);
        assert!(sb.dragging().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("src/main.rs"), "src/main.rs");
        assert_eq!(quote("my file.txt"), "'my file.txt'");
    }

    #[test]
    fn diff_classify() {
        let v = classify(["diff --git a b", "@@ -1 +1 @@", "-old", "+new", " same"].into_iter().map(String::from), true);
        assert_eq!((v.adds, v.dels), (1, 1));
        assert!(v.lines[1].0 == LineKind::Hunk);
    }
}

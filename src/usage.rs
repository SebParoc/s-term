//! AI usage tracking for Claude Code and the Antigravity CLI.
//!
//! - Claude Code: per-request token usage from session transcripts (`~/.claude/projects/**.jsonl`),
//!   read incrementally (only appended bytes). Plan rate limits (5h / weekly) and the exact context
//!   window come from `s-term statusline`, which Claude Code calls with its status-line JSON.
//! - Antigravity CLI: per-step usage stored as protobuf blobs in its SQLite conversation databases
//!   (`~/.gemini/antigravity-cli/conversations/*.db`), opened read-only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// How far back transcripts are read (Claude Code itself keeps ~30 days).
const KEEP_SECS: i64 = 400 * 86400;
const LIVE_SECS: i64 = 30 * 60;
const STATUS_FRESH_SECS: i64 = 15 * 60;
const REFRESH: Duration = Duration::from_secs(3);

// ------------------------------------------------------------------------------------------ model

#[derive(Clone)]
pub struct Rec {
    pub ts: i64,
    pub model: Arc<str>,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Rec {
    fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }
}

#[derive(Clone, Default)]
pub struct ModelTotal {
    pub model: Arc<str>,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub requests: u32,
}

impl ModelTotal {
    pub fn total(&self) -> u64 {
        self.input + self.cache_write + self.cache_read + self.output
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Window {
    FiveHours,
    Today,
    Week,
}

pub struct Live {
    /// Project folder name (Claude) or conversation title (Antigravity).
    pub label: String,
    pub model: Arc<str>,
    pub context_tokens: u64,
    /// Known window size and exact percentage (from the status line), if any.
    pub context_size: Option<u64>,
    pub context_pct: Option<f32>,
    pub last_ts: i64,
    pub requests: u32,
    pub totals: Vec<ModelTotal>,
    /// The live session belongs to the focused pane's folder.
    pub here: bool,
}

#[derive(Default)]
pub struct ToolStats {
    pub installed: bool,
    /// A process of the tool (`claude` / `agy`) is currently running.
    pub running: bool,
    pub windows: HashMap<Window, Vec<ModelTotal>>,
    /// Output tokens per hour for the last 24 hours (index 23 = current hour).
    pub hourly: [u64; 24],
    pub sessions_today: usize,
    pub live: Option<Live>,
    pub last_active: Option<i64>,
}

#[derive(Clone, Copy)]
pub struct Limit {
    pub pct: f32,
    pub resets_at: Option<i64>,
}

pub struct Limits {
    pub five_hour: Option<Limit>,
    pub seven_day: Option<Limit>,
    pub updated: i64,
}

/// Tokens of one model on one day.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct DayStat {
    pub output: u64,
    pub total: u64,
}

/// Local day number (days since 1970-01-01 in local time) -> model id -> tokens.
pub type Daily = BTreeMap<i64, BTreeMap<String, DayStat>>;

pub struct Snapshot {
    pub now: i64,
    /// Today's local day number.
    pub today: i64,
    /// Per-day usage of every model, from S-term's own ledger (outlives transcript cleanup).
    pub daily: Arc<Daily>,
    pub claude: ToolStats,
    pub agy: ToolStats,
    pub codex: ToolStats,
    pub limits: Option<Limits>,
    /// `s-term statusline` has written at least once (the hook is set up).
    pub statusline_seen: bool,
}

// ------------------------------------------------------------------------------------------ helpers

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn mtime_secs(p: &Path) -> Option<i64> {
    let t = std::fs::metadata(p).and_then(|m| m.modified()).ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// Which of the given process names (`/proc/*/comm`) are running right now.
fn running_procs(names: &[&str]) -> Vec<bool> {
    let mut found = vec![false; names.len()];
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if let Ok(comm) = std::fs::read_to_string(e.path().join("comm")) {
                if let Some(i) = names.iter().position(|n| *n == comm.trim_end()) {
                    found[i] = true;
                }
            }
        }
    }
    found
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn status_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache")).join("s-term/claude-status")
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `2026-11-23T17:41:45.674Z` -> unix seconds (UTC; offsets other than Z are ignored).
fn parse_ts(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let days = days_from_civil(n(0, 4)?, n(5, 7)?, n(8, 10)?);
    Some(days * 86400 + n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19)?)
}

fn local_offset(now: i64) -> i64 {
    // SAFETY: localtime_r only writes into the provided struct.
    unsafe {
        let t = now as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

/// Local day number of a unix time.
pub fn day_of(ts: i64) -> i64 {
    (ts + local_offset(ts)).div_euclid(86400)
}

/// (year, month 1-12, day 1-31) of a day number.
pub fn civil(day: i64) -> (i64, u32, u32) {
    let z = day + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// 0 = Monday … 6 = Sunday.
pub fn weekday(day: i64) -> u32 {
    (day + 3).rem_euclid(7) as u32
}

fn daily_from<'a>(recs: impl Iterator<Item = &'a Rec>) -> Daily {
    let mut d = Daily::new();
    for r in recs {
        let e = d.entry(day_of(r.ts)).or_default().entry(r.model.to_string()).or_default();
        e.output += r.output;
        e.total += r.input + r.cache_write + r.cache_read + r.output;
    }
    d
}

/// Daily totals persisted in `~/.local/share/s-term/usage-daily.json`.
struct Ledger {
    days: Daily,
    shared: Arc<Daily>,
}

impl Ledger {
    fn path() -> PathBuf {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share")).join("s-term/usage-daily.json")
    }

    fn load() -> Self {
        let days: Daily = std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self { shared: Arc::new(days.clone()), days }
    }

    /// Takes the larger of the stored and freshly computed value for each day/model.
    fn merge(&mut self, fresh: &Daily) {
        let mut dirty = false;
        for (day, models) in fresh {
            let slot = self.days.entry(*day).or_default();
            for (m, st) in models {
                let e = slot.entry(m.clone()).or_default();
                if st.total > e.total || st.output > e.output {
                    *e = DayStat { output: st.output.max(e.output), total: st.total.max(e.total) };
                    dirty = true;
                }
            }
        }
        if dirty {
            self.shared = Arc::new(self.days.clone());
            let p = Self::path();
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_vec(&self.days) {
                let tmp = p.with_extension("json.tmp");
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(tmp, p);
                }
            }
        }
    }
}

fn aggregate<'a>(recs: impl Iterator<Item = &'a Rec>) -> Vec<ModelTotal> {
    let mut map: HashMap<Arc<str>, ModelTotal> = HashMap::new();
    for r in recs {
        let t = map.entry(r.model.clone()).or_insert_with(|| ModelTotal { model: r.model.clone(), ..Default::default() });
        t.input += r.input;
        t.output += r.output;
        t.cache_read += r.cache_read;
        t.cache_write += r.cache_write;
        t.requests += 1;
    }
    let mut v: Vec<_> = map.into_values().collect();
    v.sort_by(|a, b| b.total().cmp(&a.total()));
    v
}

/// Shared stats over a set of sessions (each a list of records).
fn tool_stats<'a>(sessions: impl Iterator<Item = &'a [Rec]> + Clone, now: i64) -> ToolStats {
    let today = now - (now + local_offset(now)).rem_euclid(86400);
    let since = |w: Window| match w {
        Window::FiveHours => now - 5 * 3600,
        Window::Today => today,
        Window::Week => now - 7 * 86400,
    };
    let mut st = ToolStats { installed: true, ..Default::default() };
    for w in [Window::FiveHours, Window::Today, Window::Week] {
        let s = since(w);
        st.windows.insert(w, aggregate(sessions.clone().flatten().filter(|r| r.ts >= s)));
    }
    let hour0 = now - now.rem_euclid(3600) - 23 * 3600;
    for r in sessions.clone().flatten() {
        if r.ts >= hour0 {
            let i = ((r.ts - hour0) / 3600).clamp(0, 23) as usize;
            st.hourly[i] += r.output;
        }
        st.last_active = st.last_active.max(Some(r.ts));
    }
    st.sessions_today = sessions.filter(|s| s.iter().any(|r| r.ts >= today)).count();
    st
}

// ------------------------------------------------------------------------------------- Claude Code

struct ClaudeFile {
    offset: u64,
    ids: HashMap<String, usize>,
    recs: Vec<Rec>,
}

#[derive(Default)]
struct ClaudeScanner {
    files: HashMap<PathBuf, ClaudeFile>,
    models: HashMap<String, Arc<str>>,
}

fn find_jsonl(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && depth > 0 {
            find_jsonl(&p, depth - 1, out);
        } else if ft.is_file() && p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

impl ClaudeScanner {
    fn root() -> PathBuf {
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude")).join("projects")
    }

    fn scan(&mut self, now: i64) -> bool {
        let root = Self::root();
        if !root.is_dir() {
            return false;
        }
        let mut found = Vec::new();
        find_jsonl(&root, 3, &mut found);
        let mut seen = HashSet::new();
        for p in found {
            if mtime_secs(&p).is_some_and(|t| t >= now - KEEP_SECS) {
                self.update(&p);
                seen.insert(p);
            }
        }
        self.files.retain(|p, _| seen.contains(p));
        true
    }

    fn update(&mut self, path: &Path) {
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let f = self.files.entry(path.to_path_buf()).or_insert_with(|| ClaudeFile { offset: 0, ids: HashMap::new(), recs: Vec::new() });
        if len < f.offset {
            *f = ClaudeFile { offset: 0, ids: HashMap::new(), recs: Vec::new() };
        }
        if len == f.offset {
            return;
        }
        let Ok(mut file) = std::fs::File::open(path) else { return };
        if file.seek(SeekFrom::Start(f.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if file.take(256 * 1024 * 1024).read_to_end(&mut buf).is_err() {
            return;
        }
        // Only consume complete lines; a half-written last line is read next time.
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return };
        for line in buf[..end].split(|&b| b == b'\n') {
            Self::line(&mut self.models, f, line);
        }
        f.offset += end as u64 + 1;
    }

    fn line(models: &mut HashMap<String, Arc<str>>, f: &mut ClaudeFile, line: &[u8]) {
        if !line.windows(8).any(|w| w == b"\"usage\":") {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
        if v["type"] != "assistant" {
            return;
        }
        let msg = &v["message"];
        let Some(model) = msg["model"].as_str().filter(|m| !m.starts_with('<')) else { return };
        let u = &msg["usage"];
        let g = |k: &str| u[k].as_u64().unwrap_or(0);
        let model = models.entry(model.to_string()).or_insert_with(|| Arc::from(model)).clone();
        let rec = Rec {
            ts: v["timestamp"].as_str().and_then(parse_ts).unwrap_or(0),
            model,
            input: g("input_tokens"),
            output: g("output_tokens"),
            cache_read: g("cache_read_input_tokens"),
            cache_write: g("cache_creation_input_tokens"),
        };
        // A message is logged once per content block with the same usage: keep one (the latest).
        let key = format!("{}:{}", msg["id"].as_str().unwrap_or(""), v["requestId"].as_str().unwrap_or(""));
        match f.ids.get(&key) {
            Some(&i) => f.recs[i] = rec,
            None => {
                f.ids.insert(key, f.recs.len());
                f.recs.push(rec);
            }
        }
    }

    /// Claude Code's project folder name for a cwd.
    fn project_dir(cwd: &Path) -> String {
        cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
    }

    fn stats(&self, now: i64, focus: Option<&Path>, status: &HashMap<String, Value>) -> ToolStats {
        let mut st = tool_stats(self.files.values().map(|f| f.recs.as_slice()), now);
        let want = focus.map(Self::project_dir);
        // Live session: newest active one in the focused folder, else the newest active anywhere.
        let mut best: Option<(&PathBuf, &ClaudeFile, i64, bool)> = None;
        for (p, f) in &self.files {
            if p.parent().and_then(|d| d.file_name()).is_some_and(|n| n == "subagents") {
                continue;
            }
            let Some(last) = f.recs.iter().map(|r| r.ts).max() else { continue };
            if last < now - LIVE_SECS {
                continue;
            }
            let here = want.as_deref().is_some_and(|w| p.parent().and_then(|d| d.file_name()).is_some_and(|n| n == w));
            if best.is_none_or(|(_, _, t, h)| (here, last) > (h, t)) {
                best = Some((p, f, last, here));
            }
        }
        if let Some((p, f, last, here)) = best {
            let latest = f.recs.iter().max_by_key(|r| r.ts).unwrap();
            let session_id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let sl = status.get(&session_id);
            let cw = sl.map(|v| &v["context_window"]);
            let label = sl
                .and_then(|v| v["workspace"]["current_dir"].as_str().or(v["cwd"].as_str()))
                .and_then(|d| Path::new(d).file_name().map(|n| n.to_string_lossy().into_owned()))
                .or_else(|| focus.filter(|_| here).and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            st.live = Some(Live {
                label,
                model: latest.model.clone(),
                context_tokens: latest.context(),
                context_size: cw.and_then(|c| c["context_window_size"].as_u64()),
                context_pct: cw.and_then(|c| c["used_percentage"].as_f64()).map(|x| x as f32),
                last_ts: last,
                requests: f.recs.len() as u32,
                totals: aggregate(f.recs.iter()),
                here,
            });
        }
        st
    }
}

/// Latest status-line JSON per session id, plus the newest one overall.
fn read_status_files(now: i64) -> (HashMap<String, Value>, Option<Value>, bool) {
    let mut map = HashMap::new();
    let mut newest: Option<(i64, Value)> = None;
    let mut any = false;
    if let Ok(rd) = std::fs::read_dir(status_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            any = true;
            let Some(t) = mtime_secs(&p) else { continue };
            if t < now - KEEP_SECS {
                let _ = std::fs::remove_file(&p);
                continue;
            }
            let Ok(v) = std::fs::read(&p).map_err(|_| ()).and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|_| ())) else { continue };
            if !v["rate_limits"].is_null() && newest.as_ref().is_none_or(|(nt, _)| t > *nt) {
                newest = Some((t, v.clone()));
            }
            if t >= now - STATUS_FRESH_SECS {
                if let Some(id) = v["session_id"].as_str() {
                    map.insert(id.to_string(), v);
                }
            }
        }
    }
    (map, newest.map(|(t, mut v)| {
        v["_mtime"] = t.into();
        v
    }), any)
}

fn limits_from(v: &Value) -> Limits {
    let lim = |w: &Value| {
        Some(Limit { pct: w["used_percentage"].as_f64()? as f32, resets_at: w["resets_at"].as_i64().or_else(|| w["resets_at"].as_f64().map(|x| x as i64)) })
    };
    Limits {
        five_hour: lim(&v["rate_limits"]["five_hour"]),
        seven_day: lim(&v["rate_limits"]["seven_day"]),
        updated: v["_mtime"].as_i64().unwrap_or(0),
    }
}

// -------------------------------------------------------------------------------- Antigravity CLI

/// Just enough protobuf to walk fields of known messages.
mod pb {
    pub enum V<'a> {
        Int(u64),
        Bytes(&'a [u8]),
    }

    fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
        let mut r = 0u64;
        for s in (0..64).step_by(7) {
            let x = *b.get(*i)?;
            *i += 1;
            r |= ((x & 0x7f) as u64) << s;
            if x < 0x80 {
                return Some(r);
            }
        }
        None
    }

    pub fn fields(b: &[u8]) -> Vec<(u64, V<'_>)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let Some(key) = varint(b, &mut i) else { break };
            let (f, w) = (key >> 3, key & 7);
            match w {
                0 => match varint(b, &mut i) {
                    Some(v) => out.push((f, V::Int(v))),
                    None => break,
                },
                2 => {
                    let Some(n) = varint(b, &mut i) else { break };
                    let Some(s) = b.get(i..i + n as usize) else { break };
                    i += n as usize;
                    out.push((f, V::Bytes(s)));
                }
                1 => i += 8,
                5 => i += 4,
                _ => break,
            }
        }
        out
    }

    pub fn int(b: &[u8], f: u64) -> Option<u64> {
        fields(b).into_iter().find_map(|(k, v)| match v {
            V::Int(x) if k == f => Some(x),
            _ => None,
        })
    }

    pub fn msg(b: &[u8], f: u64) -> Option<&[u8]> {
        fields(b).into_iter().find_map(|(k, v)| match v {
            V::Bytes(x) if k == f => Some(x),
            _ => None,
        })
    }
}

struct AgyConv {
    stamp: (Option<i64>, Option<i64>),
    recs: Vec<Rec>,
}

#[derive(Default)]
struct AgyScanner {
    convs: HashMap<PathBuf, AgyConv>,
    models: HashMap<String, Arc<str>>,
    /// conversation id -> (title, workspace uris)
    meta: HashMap<String, (String, String)>,
    meta_stamp: Option<i64>,
}

impl AgyScanner {
    fn root() -> PathBuf {
        home().join(".gemini/antigravity-cli")
    }

    fn open(p: &Path) -> Option<rusqlite::Connection> {
        use rusqlite::OpenFlags;
        let c = rusqlite::Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
        let _ = c.busy_timeout(Duration::from_millis(250));
        Some(c)
    }

    fn intern(&mut self, s: &str) -> Arc<str> {
        self.models.entry(s.to_string()).or_insert_with(|| Arc::from(s)).clone()
    }

    fn load(&mut self, path: &Path) -> Option<Vec<Rec>> {
        let c = Self::open(path)?;
        // Model enum -> name, learned from the generation requests.
        let mut names: HashMap<u64, String> = HashMap::new();
        let mut fallback = None;
        {
            let mut st = c.prepare("select data from gen_metadata order by idx").ok()?;
            let rows = st.query_map([], |r| r.get::<_, Vec<u8>>(0)).ok()?;
            for data in rows.flatten() {
                let name = pb::msg(&data, 3).and_then(|req| pb::msg(req, 28)).and_then(|s| std::str::from_utf8(s).ok()).map(String::from);
                let id = pb::msg(&data, 1).and_then(|m| pb::msg(m, 4)).and_then(|u| pb::int(u, 1));
                if let (Some(id), Some(n)) = (id, &name) {
                    names.insert(id, n.clone());
                }
                if name.is_some() {
                    fallback = name;
                }
            }
        }
        let mut st = c.prepare("select metadata from steps order by idx").ok()?;
        let rows = st.query_map([], |r| r.get::<_, Option<Vec<u8>>>(0)).ok()?;
        let mut raw = Vec::new();
        for data in rows.flatten().flatten() {
            let Some(u) = pb::msg(&data, 9) else { continue };
            let ts = pb::msg(&data, 1).and_then(|t| pb::int(t, 1)).unwrap_or(0) as i64;
            let g = |f| pb::int(u, f).unwrap_or(0);
            let name = pb::int(u, 1).and_then(|id| names.get(&id).cloned()).or_else(|| fallback.clone()).unwrap_or_else(|| "gemini".into());
            raw.push((ts, name, g(2), g(3), g(5), g(4)));
        }
        Some(
            raw.into_iter()
                .map(|(ts, name, input, output, cache_read, cache_write)| Rec { ts, model: self.intern(&name), input, output, cache_read, cache_write })
                .collect(),
        )
    }

    fn scan(&mut self, now: i64) -> bool {
        let root = Self::root();
        if !root.is_dir() {
            return false;
        }
        let dir = root.join("conversations");
        let mut seen = HashSet::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_none_or(|x| x != "db") {
                    continue;
                }
                let wal = p.with_extension("db-wal");
                let stamp = (mtime_secs(&p), mtime_secs(&wal));
                if stamp.0.max(stamp.1).is_none_or(|t| t < now - KEEP_SECS) {
                    continue;
                }
                seen.insert(p.clone());
                if self.convs.get(&p).is_some_and(|c| c.stamp == stamp) {
                    continue;
                }
                if let Some(recs) = self.load(&p) {
                    self.convs.insert(p, AgyConv { stamp, recs });
                }
            }
        }
        self.convs.retain(|p, _| seen.contains(p));

        let sdb = root.join("conversation_summaries.db");
        let stamp = mtime_secs(&sdb).max(mtime_secs(&sdb.with_extension("db-wal")));
        if stamp != self.meta_stamp {
            self.meta_stamp = stamp;
            if let Some(c) = Self::open(&sdb) {
                if let Ok(mut st) = c.prepare("select conversation_id, title, workspace_uris from conversation_summaries") {
                    if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))) {
                        self.meta = rows.flatten().map(|(id, t, w)| (id, (t, w))).collect();
                    }
                }
            }
        }
        true
    }

    fn stats(&self, now: i64, focus: Option<&Path>) -> ToolStats {
        let mut st = tool_stats(self.convs.values().map(|c| c.recs.as_slice()), now);
        let focus_s = focus.map(|f| f.to_string_lossy().into_owned());
        let mut best: Option<(&PathBuf, &AgyConv, i64, bool)> = None;
        for (p, c) in &self.convs {
            let Some(last) = c.recs.iter().map(|r| r.ts).max() else { continue };
            if last < now - LIVE_SECS {
                continue;
            }
            let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let here = focus_s.as_ref().is_some_and(|f| self.meta.get(&id).is_some_and(|(_, w)| w.contains(f.as_str())));
            if best.is_none_or(|(_, _, t, h)| (here, last) > (h, t)) {
                best = Some((p, c, last, here));
            }
        }
        if let Some((p, c, last, here)) = best {
            let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let latest = c.recs.iter().max_by_key(|r| r.ts).unwrap();
            let label = self.meta.get(&id).map(|(t, _)| t.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| "conversation".into());
            st.live = Some(Live {
                label,
                model: latest.model.clone(),
                context_tokens: latest.context(),
                context_size: None,
                context_pct: None,
                last_ts: last,
                requests: c.recs.len() as u32,
                totals: aggregate(c.recs.iter()),
                here,
            });
        }
        st
    }
}


// ------------------------------------------------------------------------------------------- Codex

/// One Codex rollout file (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`).
#[derive(Default)]
struct CodexFile {
    offset: u64,
    recs: Vec<Rec>,
    model: Option<Arc<str>>,
    cwd: Option<String>,
    /// Running total from the last token_count event (repeats of it are skipped).
    last_total: u64,
    context_window: Option<u64>,
}

#[derive(Default)]
struct CodexScanner {
    files: HashMap<PathBuf, CodexFile>,
    models: HashMap<String, Arc<str>>,
}

impl CodexScanner {
    fn root() -> PathBuf {
        std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
    }

    fn scan(&mut self, now: i64) -> bool {
        let root = Self::root();
        if !root.is_dir() {
            return false;
        }
        let mut found = Vec::new();
        find_jsonl(&root.join("sessions"), 4, &mut found);
        let mut seen = HashSet::new();
        for p in found {
            if mtime_secs(&p).is_some_and(|t| t >= now - KEEP_SECS) {
                self.update(&p);
                seen.insert(p);
            }
        }
        self.files.retain(|p, _| seen.contains(p));
        true
    }

    fn update(&mut self, path: &Path) {
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let f = self.files.entry(path.to_path_buf()).or_default();
        if len < f.offset {
            *f = CodexFile::default();
        }
        if len == f.offset {
            return;
        }
        let Ok(mut file) = std::fs::File::open(path) else { return };
        if file.seek(SeekFrom::Start(f.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if file.take(256 * 1024 * 1024).read_to_end(&mut buf).is_err() {
            return;
        }
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return };
        for line in buf[..end].split(|&b| b == b'\n') {
            Self::line(&mut self.models, f, line);
        }
        f.offset += end as u64 + 1;
    }

    fn line(models: &mut HashMap<String, Arc<str>>, f: &mut CodexFile, line: &[u8]) {
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
        // Newer rollouts wrap each item as {timestamp, type, payload}.
        let (kind, p) = match v.get("payload") {
            Some(p) => (v["type"].as_str().unwrap_or(""), p),
            None => (v["type"].as_str().unwrap_or(""), &v),
        };
        match kind {
            "session_meta" => f.cwd = p["cwd"].as_str().map(String::from),
            "turn_context" => {
                if let Some(m) = p["model"].as_str() {
                    f.model = Some(models.entry(m.to_string()).or_insert_with(|| Arc::from(m)).clone());
                }
                if f.cwd.is_none() {
                    f.cwd = p["cwd"].as_str().map(String::from);
                }
            }
            "event_msg" if p["type"] == "token_count" => {
                let info = &p["info"];
                if info.is_null() {
                    return;
                }
                let total = info["total_token_usage"]["total_tokens"].as_u64().unwrap_or(0);
                if total == 0 || total == f.last_total {
                    return;
                }
                f.last_total = total;
                if let Some(w) = info["model_context_window"].as_u64() {
                    f.context_window = Some(w);
                }
                let u = &info["last_token_usage"];
                let g = |k: &str| u[k].as_u64().unwrap_or(0);
                let cached = g("cached_input_tokens");
                let model = f.model.clone().unwrap_or_else(|| models.entry("codex".into()).or_insert_with(|| Arc::from("codex")).clone());
                f.recs.push(Rec {
                    ts: v["timestamp"].as_str().and_then(parse_ts).unwrap_or(0),
                    model,
                    // OpenAI counts cached tokens inside input_tokens.
                    input: g("input_tokens").saturating_sub(cached),
                    output: g("output_tokens"),
                    cache_read: cached,
                    cache_write: 0,
                });
            }
            _ => {}
        }
    }

    fn stats(&self, now: i64, focus: Option<&Path>) -> ToolStats {
        let mut st = tool_stats(self.files.values().map(|f| f.recs.as_slice()), now);
        let want = focus.map(|f| f.to_string_lossy().into_owned());
        let mut best: Option<(&CodexFile, i64, bool)> = None;
        for f in self.files.values() {
            let Some(last) = f.recs.iter().map(|r| r.ts).max() else { continue };
            if last < now - LIVE_SECS {
                continue;
            }
            let here = want.is_some() && f.cwd == want;
            if best.is_none_or(|(_, t, h)| (here, last) > (h, t)) {
                best = Some((f, last, here));
            }
        }
        if let Some((f, last, here)) = best {
            let latest = f.recs.iter().max_by_key(|r| r.ts).unwrap();
            let ctx = latest.context();
            let label = f.cwd.as_deref().and_then(|c| Path::new(c).file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "session".into());
            st.live = Some(Live {
                label,
                model: latest.model.clone(),
                context_tokens: ctx,
                context_size: f.context_window,
                context_pct: f.context_window.filter(|w| *w > 0).map(|w| ctx as f32 / w as f32 * 100.0),
                last_ts: last,
                requests: f.recs.len() as u32,
                totals: aggregate(f.recs.iter()),
                here,
            });
        }
        st
    }
}

// ------------------------------------------------------------------------------------------- worker

pub struct Usage {
    shared: Arc<Mutex<Option<Arc<Snapshot>>>>,
    focus: Arc<Mutex<Option<PathBuf>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Usage {
    pub fn new(ctx: egui::Context) -> Self {
        let shared: Arc<Mutex<Option<Arc<Snapshot>>>> = Arc::default();
        let focus: Arc<Mutex<Option<PathBuf>>> = Arc::default();
        let (s2, f2) = (shared.clone(), focus.clone());
        std::thread::Builder::new()
            .name("s-term-usage".into())
            .spawn(move || {
                let mut claude = ClaudeScanner::default();
                let mut agy = AgyScanner::default();
                let mut codex = CodexScanner::default();
                let mut ledger = Ledger::load();
                loop {
                    let now = now_secs();
                    let focus = lock(&f2).clone();
                    let (status, newest, seen) = read_status_files(now);
                    let claude_installed = claude.scan(now);
                    let agy_installed = agy.scan(now);
                    let mut c = claude.stats(now, focus.as_deref(), &status);
                    c.installed = claude_installed;
                    let mut a = agy.stats(now, focus.as_deref());
                    a.installed = agy_installed;
                    let codex_installed = codex.scan(now);
                    let mut x = codex.stats(now, focus.as_deref());
                    x.installed = codex_installed;
                    let procs = running_procs(&["claude", "agy", "codex"]);
                    (c.running, a.running, x.running) = (procs[0], procs[1], procs[2]);
                    let fresh = daily_from(
                        claude
                            .files
                            .values()
                            .flat_map(|f| f.recs.iter())
                            .chain(agy.convs.values().flat_map(|c| c.recs.iter()))
                            .chain(codex.files.values().flat_map(|f| f.recs.iter())),
                    );
                    ledger.merge(&fresh);
                    let snap = Snapshot {
                        now,
                        today: day_of(now),
                        daily: ledger.shared.clone(),
                        claude: c,
                        agy: a,
                        codex: x,
                        limits: newest.as_ref().map(limits_from),
                        statusline_seen: seen,
                    };
                    *lock(&s2) = Some(Arc::new(snap));
                    ctx.request_repaint();
                    // Sleep, but wake early when the focused folder changes.
                    let until = std::time::Instant::now() + REFRESH;
                    while std::time::Instant::now() < until && *lock(&f2) == focus {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .expect("spawn usage thread");
        Self { shared, focus }
    }

    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        lock(&self.shared).clone()
    }

    pub fn set_focus(&self, cwd: Option<PathBuf>) {
        *lock(&self.focus) = cwd;
    }
}

// --------------------------------------------------------------------------------- `s-term statusline`

/// Claude Code status-line command: saves the JSON it is given (rate limits, context window)
/// for S-term to read, and prints a compact status line.
pub fn statusline_main() -> std::io::Result<()> {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let Ok(v) = serde_json::from_slice::<Value>(&input) else { return Ok(()) };
    let id: String = v["session_id"].as_str().unwrap_or("unknown").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let dir = status_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(".{id}.tmp"));
    std::fs::write(&tmp, &input)?;
    std::fs::rename(&tmp, dir.join(format!("{id}.json")))?;

    let mut parts = Vec::new();
    if let Some(m) = v["model"]["display_name"].as_str() {
        parts.push(m.to_string());
    }
    if let Some(p) = v["context_window"]["used_percentage"].as_f64() {
        parts.push(format!("ctx {p:.0}%"));
    }
    if let Some(p) = v["rate_limits"]["five_hour"]["used_percentage"].as_f64() {
        parts.push(format!("5h {p:.0}%"));
    }
    if let Some(p) = v["rate_limits"]["seven_day"]["used_percentage"].as_f64() {
        parts.push(format!("week {p:.0}%"));
    }
    println!("{}", parts.join(" · "));
    Ok(())
}

/// `s-term usage`: print the same numbers the AI tab shows (handy for checking).
pub fn report_main() {
    let now = now_secs();
    let focus = std::env::current_dir().ok();
    let (status, newest, _) = read_status_files(now);
    let mut claude = ClaudeScanner::default();
    let mut agy = AgyScanner::default();
    let mut codex = CodexScanner::default();
    let tools = [
        ("Claude Code", claude.scan(now).then(|| claude.stats(now, focus.as_deref(), &status))),
        ("Antigravity", agy.scan(now).then(|| agy.stats(now, focus.as_deref()))),
        ("Codex", codex.scan(now).then(|| codex.stats(now, focus.as_deref()))),
    ];
    for (name, st) in tools {
        let Some(st) = st else {
            println!("{name}: not found\n");
            continue;
        };
        println!("{name}  ({} sessions today)", st.sessions_today);
        if let Some(l) = &st.live {
            println!("  live: {} · {} · {} requests · context {} tokens{}", l.label, l.model, l.requests, l.context_tokens, l.context_pct.map(|p| format!(" ({p:.0}%)")).unwrap_or_default());
        }
        for (w, label) in [(Window::FiveHours, "5h"), (Window::Today, "today"), (Window::Week, "7d")] {
            for m in &st.windows[&w] {
                println!("  {label:>5}  {:<28} in {:>9}  cache-w {:>9}  cache-r {:>11}  out {:>9}  req {:>5}", m.model, m.input, m.cache_write, m.cache_read, m.output, m.requests);
            }
        }
        println!();
    }
    if let Some(v) = newest {
        let l = limits_from(&v);
        println!("Limits: 5h {:?}%  week {:?}%", l.five_hour.map(|x| x.pct), l.seven_day.map(|x| x.pct));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_token_counts() {
        let mut models = HashMap::new();
        let mut f = CodexFile::default();
        let lines = [
            r#"{"timestamp":"2026-11-23T10:00:00Z","type":"session_meta","payload":{"cwd":"/home/u/proj"}}"#,
            r#"{"timestamp":"2026-11-23T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5-codex"}}"#,
            r#"{"timestamp":"2026-11-23T10:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1200},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":600,"output_tokens":200},"model_context_window":272000}}}"#,
            // Repeated with the same total: skipped.
            r#"{"timestamp":"2026-11-23T10:00:06Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1200},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":600,"output_tokens":200}}}}"#,
            r#"{"timestamp":"2026-11-23T10:00:07Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#,
        ];
        for l in lines {
            CodexScanner::line(&mut models, &mut f, l.as_bytes());
        }
        assert_eq!(f.recs.len(), 1);
        let r = &f.recs[0];
        assert_eq!((&*r.model, r.input, r.cache_read, r.output), ("gpt-5-codex", 400, 600, 200));
        assert_eq!(r.context(), 1000);
        assert_eq!(f.context_window, Some(272000));
        assert_eq!(f.cwd.as_deref(), Some("/home/u/proj"));
    }

    #[test]
    fn timestamps() {
        assert_eq!(parse_ts("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_ts("2026-11-23T17:41:45.674Z"), Some(1795455705));
    }

    #[test]
    fn calendar() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(weekday(0), 3); // Thursday
        let d = days_from_civil(2026, 11, 24);
        assert_eq!(civil(d), (2026, 11, 24));
        assert_eq!(weekday(d), 1); // Tuesday
        assert_eq!(civil(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn claude_dedup() {
        let mut models = HashMap::new();
        let mut f = ClaudeFile { offset: 0, ids: HashMap::new(), recs: Vec::new() };
        let line = r#"{"type":"assistant","requestId":"r1","timestamp":"2026-11-23T17:41:45Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":2,"output_tokens":530,"cache_read_input_tokens":100,"cache_creation_input_tokens":50}}}"#;
        ClaudeScanner::line(&mut models, &mut f, line.as_bytes());
        ClaudeScanner::line(&mut models, &mut f, line.as_bytes());
        assert_eq!(f.recs.len(), 1);
        assert_eq!(f.recs[0].context(), 152);
        assert_eq!(&*f.recs[0].model, "claude-opus-5-5");
    }

    #[test]
    fn project_dir_name() {
        assert_eq!(ClaudeScanner::project_dir(Path::new("/home/x/s-term")), "-home-x-s-term");
        assert_eq!(ClaudeScanner::project_dir(Path::new("/a/b.c_d")), "-a-b-c-d");
    }

    #[test]
    fn protobuf_usage() {
        // field 9 { 1: 1532, 2: 3086, 3: 3479, 5: 502260 }
        let mut inner = vec![0x08];
        inner.extend([0xfc, 0x0b]); // 1532
        inner.extend([0x10, 0x8e, 0x18]); // 3086
        inner.extend([0x18, 0x97, 0x1b]); // 3479
        inner.extend([0x28, 0xf4, 0xd3, 0x1e]); // 502260
        let mut msg = vec![0x4a, inner.len() as u8];
        msg.extend(&inner);
        let u = pb::msg(&msg, 9).unwrap();
        assert_eq!(pb::int(u, 1), Some(1532));
        assert_eq!(pb::int(u, 2), Some(3086));
        assert_eq!(pb::int(u, 3), Some(3479));
        assert_eq!(pb::int(u, 5), Some(502260));
    }
}

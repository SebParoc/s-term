//! API keys for "Ask S-term".
//!
//! A key typed in Settings is saved to `~/.config/s-term/keys.json` (readable only by you), kept
//! apart from `config.json` so the config can be shared without leaking keys. When no key is
//! saved, the provider's usual environment variable is used.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::{AiProvider, Config};

fn path() -> PathBuf {
    Config::path().with_file_name("keys.json")
}

fn id(p: AiProvider) -> &'static str {
    match p {
        AiProvider::Anthropic => "anthropic",
        AiProvider::OpenAi => "openai",
        AiProvider::Gemini => "gemini",
    }
}

/// Environment variables checked for each provider, in order.
pub fn env_vars(p: AiProvider) -> &'static [&'static str] {
    match p {
        AiProvider::Anthropic => &["ANTHROPIC_API_KEY"],
        AiProvider::OpenAi => &["OPENAI_API_KEY"],
        AiProvider::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
    }
}

fn load_file() -> BTreeMap<String, String> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Where a provider's key comes from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Source {
    Saved,
    Env(&'static str),
    Missing,
}

pub fn source(p: AiProvider) -> Source {
    if load_file().get(id(p)).is_some_and(|k| !k.trim().is_empty()) {
        return Source::Saved;
    }
    for v in env_vars(p) {
        if std::env::var(v).is_ok_and(|k| !k.trim().is_empty()) {
            return Source::Env(v);
        }
    }
    Source::Missing
}

/// The key to use: the saved one, else the environment variable.
pub fn get(p: AiProvider) -> Option<String> {
    if let Some(k) = load_file().get(id(p)).map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
        return Some(k);
    }
    env_vars(p).iter().find_map(|v| std::env::var(v).ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()))
}

/// Saves (or, with an empty key, forgets) a provider's key. The file is created mode 0600.
pub fn set(p: AiProvider, key: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut all = load_file();
    if key.trim().is_empty() {
        all.remove(id(p));
    } else {
        all.insert(id(p).to_string(), key.trim().to_string());
    }
    let file = path();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(&all).unwrap_or_default())?;
    drop(f);
    std::fs::rename(tmp, file)
}

/// `sk-ant-…a1b2` style preview of a key.
pub fn masked(k: &str) -> String {
    let n = k.chars().count();
    if n <= 10 {
        return "•".repeat(n);
    }
    let head: String = k.chars().take(6).collect();
    let tail: String = k.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

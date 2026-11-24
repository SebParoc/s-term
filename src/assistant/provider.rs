//! One conversation format, sent to Anthropic, OpenAI or Gemini over plain HTTPS.
//!
//! Each assistant turn keeps the provider's raw reply and sends it back unchanged on the next
//! request to the same provider (Anthropic thinking blocks and Gemini thought signatures must be
//! returned as they came). Switching providers mid-conversation rebuilds those turns from the
//! plain text and tool calls.

use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use serde_json::{json, Value};

use crate::config::AiProvider;

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Clone, Debug)]
pub struct ToolResult {
    pub id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Clone, Debug)]
pub enum Turn {
    /// `shown` is what the user typed; `full` adds the terminal snapshot sent with it.
    User { shown: String, full: String },
    Assistant { provider: AiProvider, raw: Value, text: String, calls: Vec<ToolCall> },
    Results(Vec<ToolResult>),
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema for the arguments: `{"type": "object", "properties": {...}, "required": [...]}`.
    pub schema: Value,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout(Duration::from_secs(300)).user_agent(concat!("s-term/", env!("CARGO_PKG_VERSION"))).build()
}

/// Turns an HTTP error into a short message, using the API's own error text when there is one.
fn http_err(e: ureq::Error, who: &str) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(String::from).or_else(|| v["error"].as_str().map(String::from)))
                .unwrap_or_else(|| body.chars().take(300).collect());
            let mut msg = msg.trim().to_string();
            if !msg.ends_with(['.', '!', '?']) {
                msg.push('.');
            }
            let bad_key = matches!(code, 401 | 403) || msg.contains("API key not valid");
            let hint = if bad_key {
                " Check the API key in Settings → Assistant."
            } else if code == 429 {
                " Rate limited or out of credit."
            } else {
                ""
            };
            anyhow!("{who} returned {code}: {msg}{hint}")
        }
        ureq::Error::Transport(t) => anyhow!("Couldn't reach {who}: {t}"),
    }
}

fn post(url: &str, headers: &[(&str, &str)], body: &Value, who: &str) -> Result<Value> {
    let mut req = agent().post(url).set("content-type", "application/json");
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req.send_json(body).map_err(|e| http_err(e, who))?;
    resp.into_json::<Value>().with_context(|| format!("{who} sent a reply S-term couldn't read"))
}

fn get(url: &str, headers: &[(&str, &str)], who: &str) -> Result<Value> {
    let mut req = agent().get(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req.call().map_err(|e| http_err(e, who))?;
    resp.into_json::<Value>().with_context(|| format!("{who} sent a reply S-term couldn't read"))
}

/// Sends the conversation and returns the assistant's next turn.
pub fn send(p: AiProvider, key: &str, model: &str, system: &str, history: &[Turn], tools: &[ToolDef]) -> Result<Turn> {
    match p {
        AiProvider::Anthropic => anthropic(key, model, system, history, tools),
        AiProvider::OpenAi => openai(key, model, system, history, tools),
        AiProvider::Gemini => gemini(key, model, system, history, tools),
    }
}

/// Models the key can use, best guesses first.
pub fn list_models(p: AiProvider, key: &str) -> Result<Vec<String>> {
    match p {
        AiProvider::Anthropic => {
            let v = get("https://api.anthropic.com/v1/models?limit=100", &[("x-api-key", key), ("anthropic-version", ANTHROPIC_VERSION)], "Anthropic")?;
            // Newest first, as the API returns them.
            Ok(v["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        }
        AiProvider::OpenAi => {
            let v = get("https://api.openai.com/v1/models", &[("authorization", &format!("Bearer {key}"))], "OpenAI")?;
            let skip = ["audio", "realtime", "tts", "transcribe", "image", "search", "embedding", "moderation", "instruct", "dall-e", "whisper", "codex", "babbage", "davinci", "computer-use", "deep-research"];
            let mut ms: Vec<(i64, String)> = v["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| Some((m["created"].as_i64().unwrap_or(0), m["id"].as_str()?.to_string())))
                .filter(|(_, id)| (id.starts_with("gpt-") || id.starts_with('o')) && !skip.iter().any(|s| id.contains(s)))
                .collect();
            ms.sort_by(|a, b| b.0.cmp(&a.0));
            Ok(ms.into_iter().map(|(_, id)| id).collect())
        }
        AiProvider::Gemini => {
            let v = get("https://generativelanguage.googleapis.com/v1beta/models?pageSize=200", &[("x-goog-api-key", key)], "Gemini")?;
            let mut ms: Vec<String> = v["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["supportedGenerationMethods"].as_array().is_some_and(|a| a.iter().any(|x| x == "generateContent")))
                .filter_map(|m| m["name"].as_str().map(|n| n.trim_start_matches("models/").to_string()))
                .filter(|n| n.starts_with("gemini-") && !n.contains("embedding") && !n.contains("image") && !n.contains("tts") && !n.contains("audio"))
                .collect();
            // Higher version numbers first; "pro" before "flash" within a version.
            ms.sort_by(|a, b| version_key(b).partial_cmp(&version_key(a)).unwrap_or(std::cmp::Ordering::Equal));
            Ok(ms)
        }
    }
}

/// `gemini-3.5-pro` -> (3.5, 2): version, then tier.
fn version_key(id: &str) -> (f32, u8, bool) {
    let rest = id.trim_start_matches("gemini-");
    let ver = rest.split('-').next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    let tier = if rest.contains("pro") { 3 } else if rest.contains("flash-lite") { 1 } else if rest.contains("flash") { 2 } else { 0 };
    // Stable releases (no "preview"/"exp") rank above previews of the same version.
    (ver, tier, !(rest.contains("preview") || rest.contains("exp")))
}

// ------------------------------------------------------------------------------------- Anthropic

const ANTHROPIC_VERSION: &str = "2026-10-21";

/// Models that support server-side refusal fallbacks.
fn has_fallbacks(model: &str) -> bool {
    ["claude-fable-5", "claude-mythos-5", "claude-opus-5", "claude-sonnet-5-5"].iter().any(|p| model.starts_with(p))
}

fn anthropic(key: &str, model: &str, system: &str, history: &[Turn], tools: &[ToolDef]) -> Result<Turn> {
    let mut messages = Vec::new();
    for t in history {
        match t {
            Turn::User { full, .. } => messages.push(json!({"role": "user", "content": full})),
            Turn::Assistant { provider: AiProvider::Anthropic, raw, .. } => messages.push(json!({"role": "assistant", "content": raw})),
            Turn::Assistant { text, calls, .. } => {
                let mut content = Vec::new();
                if !text.is_empty() {
                    content.push(json!({"type": "text", "text": text}));
                }
                for c in calls {
                    content.push(json!({"type": "tool_use", "id": safe_id(&c.id), "name": c.name, "input": c.args}));
                }
                messages.push(json!({"role": "assistant", "content": content}));
            }
            Turn::Results(rs) => {
                let content: Vec<Value> = rs.iter().map(|r| json!({"type": "tool_result", "tool_use_id": safe_id(&r.id), "content": r.content, "is_error": r.is_error})).collect();
                messages.push(json!({"role": "user", "content": content}));
            }
        }
    }
    let tools: Vec<Value> = tools.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema})).collect();
    let mut body = json!({
        "model": model,
        "max_tokens": 16000,
        "system": [{"type": "text", "text": system}],
        "tools": tools,
        "messages": messages,
        // Caches the stable prefix (tools, system prompt, earlier turns) between requests.
        "cache_control": {"type": "ephemeral"},
    });
    let mut headers = vec![("x-api-key", key), ("anthropic-version", ANTHROPIC_VERSION)];
    if has_fallbacks(model) {
        body["fallbacks"] = json!("default");
        headers.push(("anthropic-beta", "server-side-fallback-2026-07-01"));
    }
    let v = post("https://api.anthropic.com/v1/messages", &headers, &body, "Anthropic")?;
    let content = v["content"].clone();
    let mut text = String::new();
    let mut calls = Vec::new();
    for b in content.as_array().into_iter().flatten() {
        match b["type"].as_str() {
            Some("text") => {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(b["text"].as_str().unwrap_or(""));
            }
            Some("tool_use") => calls.push(ToolCall { id: b["id"].as_str().unwrap_or("").into(), name: b["name"].as_str().unwrap_or("").into(), args: b["input"].clone() }),
            _ => {}
        }
    }
    match v["stop_reason"].as_str() {
        Some("refusal") => bail!("The model declined this request."),
        Some("max_tokens") if !calls.is_empty() => bail!("The reply was cut off before the model finished a tool call."),
        _ => {}
    }
    Ok(Turn::Assistant { provider: AiProvider::Anthropic, raw: content, text, calls })
}

/// Anthropic tool ids must match `[a-zA-Z0-9_-]+`.
fn safe_id(id: &str) -> String {
    let s: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    if s.is_empty() { "call".into() } else { s }
}

// ---------------------------------------------------------------------------------------- OpenAI

fn openai(key: &str, model: &str, system: &str, history: &[Turn], tools: &[ToolDef]) -> Result<Turn> {
    let mut messages = vec![json!({"role": "system", "content": system})];
    for t in history {
        match t {
            Turn::User { full, .. } => messages.push(json!({"role": "user", "content": full})),
            Turn::Assistant { provider, raw, text, calls } => {
                let tool_calls: Vec<Value> = if *provider == AiProvider::OpenAi && raw["tool_calls"].is_array() {
                    raw["tool_calls"].as_array().cloned().unwrap_or_default()
                } else {
                    calls.iter().map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.args.to_string()}})).collect()
                };
                let mut m = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
                if !tool_calls.is_empty() {
                    m["tool_calls"] = json!(tool_calls);
                }
                messages.push(m);
            }
            Turn::Results(rs) => {
                for r in rs {
                    messages.push(json!({"role": "tool", "tool_call_id": r.id, "content": r.content}));
                }
            }
        }
    }
    let tools: Vec<Value> = tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}})).collect();
    let body = json!({"model": model, "messages": messages, "tools": tools, "tool_choice": "auto"});
    let auth = format!("Bearer {key}");
    let v = post("https://api.openai.com/v1/chat/completions", &[("authorization", &auth)], &body, "OpenAI")?;
    let msg = v["choices"][0]["message"].clone();
    if let Some(r) = msg["refusal"].as_str().filter(|r| !r.is_empty()) {
        bail!("The model declined: {r}");
    }
    let text = msg["content"].as_str().unwrap_or("").to_string();
    let calls = msg["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| ToolCall {
            id: c["id"].as_str().unwrap_or("").into(),
            name: c["function"]["name"].as_str().unwrap_or("").into(),
            args: c["function"]["arguments"].as_str().and_then(|a| serde_json::from_str(a).ok()).unwrap_or(json!({})),
        })
        .collect();
    Ok(Turn::Assistant { provider: AiProvider::OpenAi, raw: msg, text, calls })
}

// ---------------------------------------------------------------------------------------- Gemini

fn gemini(key: &str, model: &str, system: &str, history: &[Turn], tools: &[ToolDef]) -> Result<Turn> {
    let mut contents = Vec::new();
    for t in history {
        match t {
            Turn::User { full, .. } => contents.push(json!({"role": "user", "parts": [{"text": full}]})),
            Turn::Assistant { provider: AiProvider::Gemini, raw, .. } => contents.push(json!({"role": "model", "parts": raw})),
            Turn::Assistant { text, calls, .. } => {
                let mut parts = Vec::new();
                if !text.is_empty() {
                    parts.push(json!({"text": text}));
                }
                for c in calls {
                    parts.push(json!({"functionCall": {"name": c.name, "args": c.args}}));
                }
                contents.push(json!({"role": "model", "parts": parts}));
            }
            Turn::Results(rs) => {
                let parts: Vec<Value> = rs
                    .iter()
                    .map(|r| {
                        let key = if r.is_error { "error" } else { "result" };
                        json!({"functionResponse": {"name": r.name, "response": {key: r.content}}})
                    })
                    .collect();
                contents.push(json!({"role": "user", "parts": parts}));
            }
        }
    }
    let decls: Vec<Value> = tools
        .iter()
        .map(|t| {
            // Gemini rejects an object schema with no properties: leave `parameters` out instead.
            if t.schema["properties"].as_object().is_some_and(|p| p.is_empty()) {
                json!({"name": t.name, "description": t.description})
            } else {
                json!({"name": t.name, "description": t.description, "parameters": t.schema})
            }
        })
        .collect();
    let body = json!({
        "systemInstruction": {"parts": [{"text": system}]},
        "contents": contents,
        "tools": [{"functionDeclarations": decls}],
    });
    let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");
    let v = post(&url, &[("x-goog-api-key", key)], &body, "Gemini")?;
    let cand = &v["candidates"][0];
    if cand.is_null() {
        let why = v["promptFeedback"]["blockReason"].as_str().unwrap_or("no reply");
        bail!("Gemini returned no answer ({why}).");
    }
    let parts = cand["content"]["parts"].clone();
    let mut text = String::new();
    let mut calls = Vec::new();
    for (i, p) in parts.as_array().into_iter().flatten().enumerate() {
        if p["thought"].as_bool() == Some(true) {
            continue;
        }
        if let Some(t) = p["text"].as_str() {
            text.push_str(t);
        }
        if let Some(fc) = p.get("functionCall") {
            let id = fc["id"].as_str().map(String::from).unwrap_or_else(|| format!("g{i}"));
            calls.push(ToolCall { id, name: fc["name"].as_str().unwrap_or("").into(), args: fc["args"].clone() });
        }
    }
    if text.is_empty() && calls.is_empty() {
        let why = cand["finishReason"].as_str().unwrap_or("empty reply");
        bail!("Gemini returned no answer ({why}).");
    }
    Ok(Turn::Assistant { provider: AiProvider::Gemini, raw: if parts.is_array() { parts } else { json!([]) }, text, calls })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_versions_sort_newest_pro_first() {
        let mut v = vec!["gemini-2.5-flash", "gemini-3-pro-preview", "gemini-2.5-pro", "gemini-3-flash"];
        v.sort_by(|a, b| version_key(b).partial_cmp(&version_key(a)).unwrap());
        assert_eq!(v, ["gemini-3-pro-preview", "gemini-3-flash", "gemini-2.5-pro", "gemini-2.5-flash"]);
    }

    /// Hits the real APIs with an invalid key: checks URLs, headers and error parsing.
    /// Run with `cargo test -- --ignored live_`.
    #[test]
    #[ignore]
    fn live_bad_key_errors_are_readable() {
        let tools = vec![ToolDef { name: "list_panes", description: "List panes.", schema: json!({"type": "object", "properties": {}, "required": []}) }];
        let history = vec![Turn::User { shown: "hi".into(), full: "hi".into() }];
        for (p, model) in [(AiProvider::Anthropic, "claude-opus-5-5"), (AiProvider::OpenAi, "gpt-5"), (AiProvider::Gemini, "gemini-2.5-flash")] {
            let e = send(p, "invalid-key-for-test", model, "test", &history, &tools).unwrap_err().to_string();
            println!("{p:?} send: {e}");
            assert!(e.contains(p.name()), "{e}");
            let e = list_models(p, "invalid-key-for-test").unwrap_err().to_string();
            println!("{p:?} models: {e}");
        }
    }

    #[test]
    fn anthropic_ids_are_sanitized() {
        assert_eq!(safe_id("call_abc-1"), "call_abc-1");
        assert_eq!(safe_id("g:0"), "g_0");
        assert_eq!(safe_id(""), "call");
    }
}

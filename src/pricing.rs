//! API list prices, so the AI view can show what the tokens would have cost on each provider's
//! pay-as-you-go API, in USD. Plans (Claude Pro/Max, ChatGPT, Google AI) bill differently, so it's
//! only an equivalent. Rates are USD per million tokens as published on 2026-11-24:
//!
//! - Anthropic: <https://platform.claude.com/docs/en/about-claude/pricing>
//! - OpenAI: <https://developers.openai.com/api/docs/pricing> (Standard tier)
//! - Google: <https://aistudio.google.com/docs/pricing> (Standard paid tier)
//!
//! Each request is priced when it's read, at the rates of its own time and prompt length, because
//! some prices depend on them (Claude Haiku 5.5 over 100k prompt tokens, GPT over 272k, Gemini Pro
//! over 200k; Gemini 4 Carbon's introductory price ends on 2026-12-01).

use crate::usage::{model_key, Rec};

/// Claude Code's fast mode costs twice the standard price, on every kind of token.
pub const FAST: f64 = 2.0;
/// US-only inference (`inference_geo: "us"`) costs 1.1 times the standard price.
pub const US_ONLY: f64 = 1.1;

/// USD per million tokens.
#[derive(Clone, Copy)]
struct Rates {
    input: f64,
    write_5m: f64,
    write_1h: f64,
    read: f64,
    output: f64,
}

/// Anthropic's rates: input, 5-minute and 1-hour cache writes, cache reads, output.
const fn claude(input: f64, write_5m: f64, write_1h: f64, read: f64, output: f64) -> Rates {
    Rates { input, write_5m, write_1h, read, output }
}

/// OpenAI's and Google's rates: input, cached input, output. Their caching is implicit and writes
/// aren't billed apart from input (nor reported), so a cache write costs what input does.
const fn cached(input: f64, read: f64, output: f64) -> Rates {
    Rates { input, write_5m: input, write_1h: input, read, output }
}

/// One model's prices. `key` is a `model_key` ("claude-opus-5-5", "gpt-5-3-codex"); `from` is the
/// unix time they apply from (0 = always); `long` holds the prices for a request whose prompt (all
/// its input tokens, cached or not) has more than that many tokens.
struct Price {
    key: &'static str,
    from: i64,
    base: Rates,
    long: Option<(u64, Rates)>,
}

const fn p(key: &'static str, base: Rates) -> Price {
    Price { key, from: 0, base, long: None }
}

const fn tiered(key: &'static str, base: Rates, over: u64, long: Rates) -> Price {
    Price { key, from: 0, base, long: Some((over, long)) }
}

const fn since(key: &'static str, from: i64, base: Rates) -> Price {
    Price { key, from, base, long: None }
}

/// 2026-12-01 and 2027-01-01, 00:00 UTC.
const DEC_2026: i64 = 1_796_083_200;
const JAN_2027: i64 = 1_798_761_600;

const OPUS_5: Rates = claude(5.0, 6.25, 10.0, 0.50, 25.0);
const OPUS_4: Rates = claude(15.0, 18.75, 30.0, 1.50, 75.0);
const SONNET_4: Rates = claude(3.0, 3.75, 6.0, 0.30, 15.0);
const FABLE_5_1: Rates = claude(10.0, 12.5, 20.0, 0.25, 50.0);
const FABLE_5: Rates = claude(10.0, 12.5, 20.0, 1.0, 50.0);
const FABLE_5_2: Rates = claude(8.0, 10.0, 16.0, 0.20, 40.0);

#[rustfmt::skip]
static PRICES: &[Price] = &[
    // ---- Anthropic
    p("claude-fable-5-2", FABLE_5_2),
    p("claude-mythos-5-2", FABLE_5_2),
    p("claude-fable-5-1", FABLE_5_1),
    p("claude-mythos-5-1", FABLE_5_1),
    p("claude-fable-5", FABLE_5),
    p("claude-mythos-5", FABLE_5),
    p("claude-opus-5-5", claude(4.0, 5.0, 8.0, 0.20, 20.0)),
    p("claude-opus-5", OPUS_5),
    p("claude-opus-4-8", OPUS_5),
    p("claude-opus-4-7", OPUS_5),
    p("claude-opus-4-6", OPUS_5),
    p("claude-opus-4-5", OPUS_5),
    p("claude-opus-4-1", OPUS_4),
    p("claude-opus-4", OPUS_4),
    p("claude-sonnet-5-5", claude(2.0, 2.5, 4.0, 0.10, 10.0)),
    p("claude-sonnet-5", claude(2.0, 2.5, 4.0, 0.20, 10.0)),
    p("claude-sonnet-4-6", SONNET_4),
    p("claude-sonnet-4-5", SONNET_4),
    p("claude-sonnet-4", SONNET_4),
    tiered("claude-haiku-5-5", claude(0.10, 0.125, 0.20, 0.01, 0.50), 100_000, claude(0.50, 0.625, 1.0, 0.05, 2.50)),
    p("claude-haiku-4-5", claude(1.0, 1.25, 2.0, 0.10, 5.0)),
    // ---- OpenAI (Codex)
    tiered("gpt-6-2-astra", cached(8.0, 0.40, 40.0), 272_000, cached(16.0, 0.80, 60.0)),
    tiered("gpt-6-2-sol", cached(2.0, 0.10, 10.0), 272_000, cached(4.0, 0.20, 15.0)),
    tiered("gpt-6-1-sol", cached(2.0, 0.10, 10.0), 272_000, cached(4.0, 0.20, 15.0)),
    tiered("gpt-6-1-terra", cached(1.0, 0.05, 5.0), 272_000, cached(2.0, 0.10, 7.5)),
    tiered("gpt-6-astra", cached(10.0, 1.0, 50.0), 272_000, cached(20.0, 2.0, 75.0)),
    tiered("gpt-6-sol", cached(2.0, 0.20, 10.0), 272_000, cached(4.0, 0.40, 15.0)),
    tiered("gpt-6-luna", cached(0.10, 0.01, 0.50), 272_000, cached(0.20, 0.02, 0.75)),
    tiered("gpt-5-6-sol", cached(4.0, 0.40, 20.0), 272_000, cached(8.0, 0.80, 30.0)),
    tiered("gpt-5-6-terra", cached(2.0, 0.20, 12.0), 272_000, cached(4.0, 0.40, 18.0)),
    tiered("gpt-5-6-luna", cached(0.20, 0.02, 1.20), 272_000, cached(0.40, 0.04, 1.80)),
    tiered("gpt-5-5-pro", cached(30.0, 30.0, 180.0), 272_000, cached(60.0, 60.0, 270.0)),
    tiered("gpt-5-5", cached(5.0, 0.50, 30.0), 272_000, cached(10.0, 1.0, 45.0)),
    tiered("gpt-5-4-pro", cached(30.0, 30.0, 180.0), 272_000, cached(60.0, 60.0, 270.0)),
    tiered("gpt-5-4", cached(2.5, 0.25, 15.0), 272_000, cached(5.0, 0.50, 22.5)),
    p("gpt-5-4-mini", cached(0.75, 0.075, 4.5)),
    p("gpt-5-4-nano", cached(0.20, 0.02, 1.25)),
    p("gpt-5-3-codex", cached(1.75, 0.175, 14.0)),
    p("gpt-5-2-pro", cached(21.0, 21.0, 168.0)),
    p("gpt-5-2", cached(1.75, 0.175, 14.0)),
    // No longer on the page; this is its last listed price (it was priced like gpt-5-mini).
    p("gpt-5-1-codex-mini", cached(0.25, 0.025, 2.0)),
    p("gpt-5-1", cached(1.25, 0.125, 10.0)),
    p("gpt-5-pro", cached(15.0, 15.0, 120.0)),
    p("gpt-5-mini", cached(0.25, 0.025, 2.0)),
    p("gpt-5-nano", cached(0.05, 0.005, 0.40)),
    p("gpt-5", cached(1.25, 0.125, 10.0)),
    // ---- Google (Antigravity)
    p("gemini-4-neon", cached(3.0, 0.15, 15.0)),
    p("gemini-4-hydrogen", cached(1.5, 0.075, 7.5)),
    p("gemini-4-carbon", cached(2.0, 0.10, 10.0)),
    since("gemini-4-carbon", DEC_2026, cached(4.0, 0.20, 20.0)),
    p("gemini-4-argon", cached(2.0, 0.10, 10.0)),
    since("gemini-4-argon", DEC_2026, cached(4.0, 0.20, 20.0)),
    p("gemini-3-8-flash", cached(0.75, 0.075, 3.75)),
    since("gemini-3-8-flash", JAN_2027, cached(1.5, 0.15, 7.5)),
    p("gemini-3-6-flash", cached(0.75, 0.075, 3.75)),
    since("gemini-3-6-flash", JAN_2027, cached(1.5, 0.15, 7.5)),
    p("gemini-3-5-flash-lite", cached(0.30, 0.03, 2.50)),
    tiered("gemini-3-1-pro", cached(2.0, 0.20, 12.0), 200_000, cached(4.0, 0.40, 18.0)),
    p("gemini-3-1-flash-lite", cached(0.25, 0.025, 1.50)),
    p("gemini-3-flash", cached(0.50, 0.05, 3.0)),
    tiered("gemini-2-5-pro", cached(1.25, 0.125, 10.0), 200_000, cached(2.5, 0.25, 15.0)),
    p("gemini-2-5-flash-lite", cached(0.10, 0.01, 0.40)),
    p("gemini-2-5-flash", cached(0.30, 0.03, 2.50)),
];

/// What tokens cost on the API, by kind of token (USD).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Cost {
    pub input: f64,
    pub cache_write: f64,
    pub cache_read: f64,
    pub output: f64,
}

impl Cost {
    pub fn total(&self) -> f64 {
        self.input + self.cache_write + self.cache_read + self.output
    }
}

impl std::ops::AddAssign for Cost {
    fn add_assign(&mut self, o: Cost) {
        self.input += o.input;
        self.cache_write += o.cache_write;
        self.cache_read += o.cache_read;
        self.output += o.output;
    }
}

/// `key` names the model `id`: `id` is `key`, or `key` and a suffix that isn't another version
/// ("-20250514", "-high", "-codex", "-preview" fit; "claude-opus-5" doesn't name "claude-opus-5-5").
fn names(id: &str, key: &str) -> bool {
    match id.strip_prefix(key) {
        Some("") => true,
        Some(rest) => rest.strip_prefix('-').is_some_and(|r| {
            let seg = r.split('-').next().unwrap_or("");
            !(seg.bytes().all(|b| b.is_ascii_digit()) && seg.len() < 8)
        }),
        None => false,
    }
}

/// The prices of `model` at time `ts`: the longest key that names it, its latest dated prices.
fn find(model: &str, ts: i64) -> Option<&'static Price> {
    let id = model_key(model);
    PRICES.iter().filter(|p| p.from <= ts && names(&id, p.key)).max_by_key(|p| (p.key.len(), p.from))
}

/// Whether there's an API price for `model`.
#[cfg(test)]
fn known(model: &str) -> bool {
    find(model, i64::MAX).is_some()
}

/// What request `r` would cost on the API, times `mult` ([`FAST`], [`US_ONLY`]). `write_1h` is the
/// part of `r.cache_write` that went to the 1-hour cache (Claude Code writes all of it there).
/// None for a model with no known price.
pub fn request(r: &Rec, write_1h: u64, mult: f64) -> Option<Cost> {
    let p = find(&r.model, r.ts)?;
    let prompt = r.input + r.cache_read + r.cache_write;
    let rt = match p.long {
        Some((over, long)) if prompt > over => long,
        _ => p.base,
    };
    let m = mult / 1e6;
    let w1h = write_1h.min(r.cache_write);
    Some(Cost {
        input: r.input as f64 * rt.input * m,
        cache_write: ((r.cache_write - w1h) as f64 * rt.write_5m + w1h as f64 * rt.write_1h) * m,
        cache_read: r.cache_read as f64 * rt.read * m,
        output: r.output as f64 * rt.output * m,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn rec(model: &str, ts: i64, input: u64, output: u64, cache_read: u64, cache_write: u64) -> Rec {
        Rec { ts, model: Arc::from(model), input, output, cache_read, cache_write, cost: None }
    }

    fn total(model: &str, ts: i64, t: (u64, u64, u64, u64), write_1h: u64) -> Option<f64> {
        request(&rec(model, ts, t.0, t.1, t.2, t.3), write_1h, 1.0).map(|c| (c.total() * 1e6).round() / 1e6)
    }

    #[test]
    fn claude_rates() {
        let m = 1_000_000;
        // Opus 5.5: $4 input, $20 output, $0.20 cache reads, $8 1-hour writes.
        assert_eq!(total("claude-opus-5-5", 0, (m, m, m, m), m), Some(4.0 + 20.0 + 0.2 + 8.0));
        // Without the 1-hour split a write is a 5-minute one ($5).
        assert_eq!(total("claude-opus-5-5", 0, (0, 0, 0, m), 0), Some(5.0));
        // Fast mode doubles everything.
        let fast = request(&rec("claude-opus-5-5", 0, m, m, 0, 0), 0, FAST).unwrap().total();
        assert!((fast - 48.0).abs() < 1e-9);
        // Haiku 5.5 is priced by prompt length, cache reads and writes included.
        assert_eq!(total("claude-haiku-5-5", 0, (50_000, m, 50_000, 0), 0), Some(0.005 + 0.5 + 0.0005));
        assert_eq!(total("claude-haiku-5-5", 0, (50_000, m, 50_001, 0), 0), Some(2.5275)); // 0.025 + 2.5 + 0.0025

    }

    #[test]
    fn model_ids() {
        // A dated snapshot is its model; the next version isn't.
        assert!(std::ptr::eq(find("claude-opus-4-20250514", 0).unwrap(), find("claude-opus-4", 0).unwrap()));
        assert_eq!(find("claude-opus-4-1-20250805", 0).unwrap().key, "claude-opus-4-1");
        assert_eq!(find("claude-opus-5-5", 0).unwrap().key, "claude-opus-5-5");
        assert_eq!(find("claude-sonnet-4-5-20250929", 0).unwrap().key, "claude-sonnet-4-5");
        assert_eq!(find("Claude Opus 5.5 (Thinking)", 0).unwrap().key, "claude-opus-5-5");
        // Codex and effort suffixes fall back to the base model; unknown versions get no price.
        assert_eq!(find("gpt-5.2-codex", 0).unwrap().key, "gpt-5-2");
        assert_eq!(find("gpt-5.1-codex-mini", 0).unwrap().key, "gpt-5-1-codex-mini");
        assert_eq!(find("gemini-4-neon-high", 0).unwrap().key, "gemini-4-neon");
        assert_eq!(find("gemini-3.1-pro-preview", 0).unwrap().key, "gemini-3-1-pro");
        assert!(!known("gpt-5.7"));
        assert!(!known("claude-opus-6"));
        assert!(!known("gemini"));
        assert!(!known("codex"));
    }

    #[test]
    fn dated_and_long_prices() {
        let m = 1_000_000;
        assert_eq!(total("gemini-4-carbon", DEC_2026 - 1, (0, m, 0, 0), 0), Some(10.0));
        assert_eq!(total("gemini-4-carbon", DEC_2026, (0, m, 0, 0), 0), Some(20.0));
        // GPT-5.5 above 272k prompt tokens (uncached + cached).
        assert_eq!(total("gpt-5.5", 0, (200_000, 0, 72_000, 0), 0), Some(1.0 + 0.036));
        assert_eq!(total("gpt-5.5", 0, (200_001, 0, 72_000, 0), 0), Some(2.00001 + 0.072));
    }
}

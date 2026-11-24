//! Drawing for the AI usage tab and the status-bar meter.

use std::f32::consts::{PI, TAU};

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Mesh, Painter, Pos2, Rect, RichText, Sense, Shape, Stroke};

use crate::theme::{c32, lerp_color, Theme};
use crate::config::{Config, HeatMetric};
use crate::usage::{civil, weekday, DayStat, Limit, Live, ModelTotal, Snapshot, ToolStats, Window};

const CLICK: Sense = Sense { click: true, drag: false, focusable: false };

pub const CLAUDE_ORANGE: Color32 = Color32::from_rgb(0xd9, 0x77, 0x57);
const GEMINI_BLUE: Color32 = Color32::from_rgb(0x42, 0x85, 0xf4);
const GEMINI_PURPLE: Color32 = Color32::from_rgb(0x9b, 0x72, 0xcb);
const GEMINI_PINK: Color32 = Color32::from_rgb(0xd9, 0x65, 0x70);

pub struct UsageUi {
    window: Window,
    /// Heatmap filter: None = all models.
    heat_model: Option<String>,
}

impl Default for UsageUi {
    fn default() -> Self {
        Self { window: Window::Today, heat_model: None }
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn day_label(day: i64) -> String {
    let (y, m, d) = civil(day);
    format!("{}, {} {d} {y}", WEEKDAYS[weekday(day) as usize], MONTHS[(m - 1) as usize])
}

fn metric_of(st: &DayStat, m: HeatMetric) -> u64 {
    match m {
        HeatMetric::Output => st.output,
        HeatMetric::Total => st.total,
    }
}

/// GitHub-style calendar of daily usage. In "All" mode each day takes the color of its busiest model.
fn heatmap(ui: &mut egui::Ui, theme: &Theme, s: &Snapshot, state: &mut UsageUi, metric: &mut HeatMetric) {
    let horizon = s.today - 371;
    let mut totals: Vec<(String, u64)> = {
        let mut m: std::collections::HashMap<&str, u64> = Default::default();
        for (_, models) in s.daily.range(horizon..=s.today) {
            for (k, v) in models {
                *m.entry(k.as_str()).or_default() += metric_of(v, *metric);
            }
        }
        m.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    };
    totals.sort_by(|a, b| b.1.cmp(&a.1));
    if state.heat_model.as_ref().is_some_and(|m| !totals.iter().any(|(k, _)| k == m)) {
        state.heat_model = None;
    }

    section(ui, theme, "ACTIVITY", |ui| {
        for (m, label) in [(HeatMetric::Total, "all"), (HeatMetric::Output, "output")] {
            if text_toggle(ui, theme, label, None, *metric == m, Id::new(("s-term-heat-metric", label))).clicked() {
                *metric = m;
            }
        }
    });

    let value = |day: i64| -> (u64, Option<&str>) {
        let Some(models) = s.daily.get(&day) else { return (0, None) };
        match &state.heat_model {
            Some(m) => (models.get(m).map_or(0, |v| metric_of(v, *metric)), Some(m.as_str())),
            None => {
                let sum = models.values().map(|v| metric_of(v, *metric)).sum();
                let top = models.iter().max_by_key(|(_, v)| metric_of(v, *metric)).map(|(k, _)| k.as_str());
                (sum, top)
            }
        }
    };

    let label_w = 28.0;
    let (cell, gap) = (11.0, 3.0);
    let avail = ui.available_width();
    let weeks = (((avail - label_w + gap) / (cell + gap)).floor() as i64).clamp(6, 53);
    let grid_w = label_w + weeks as f32 * (cell + gap) - gap;
    let (rect, resp) = ui.allocate_exact_size(vec2(grid_w.min(avail), 16.0 + 7.0 * (cell + gap)), Sense::hover());
    let p = ui.painter();
    let end_monday = s.today - weekday(s.today) as i64;
    let start = end_monday - (weeks - 1) * 7;

    let mut vals: Vec<u64> = (start..=s.today).map(|d| value(d).0).filter(|&v| v > 0).collect();
    vals.sort_unstable();
    let q = |f: f32| vals.get(((vals.len() as f32 - 1.0) * f).round() as usize).copied().unwrap_or(0);
    let (q1, q2, q3) = (q(0.25), q(0.5), q(0.75));
    let level = |v: u64| match v {
        0 => 0,
        v if v <= q1 => 1,
        v if v <= q2 => 2,
        v if v <= q3 => 3,
        _ => 4,
    };
    let empty = lerp_color(theme.header, theme.border, 0.8);
    let shade = |lvl: usize, c: Color32| if lvl == 0 { empty } else { lerp_color(empty, c, [0.0, 0.32, 0.55, 0.78, 1.0][lvl]) };

    for (i, name) in [(0usize, "Mon"), (2, "Wed"), (4, "Fri")] {
        p.text(pos2(rect.min.x, rect.min.y + 16.0 + i as f32 * (cell + gap) + cell / 2.0), Align2::LEFT_CENTER, name, FontId::proportional(9.5), theme.muted);
    }
    let mut last_month = 0;
    let mut last_label_col = -10i64;
    let mut hovered: Option<i64> = None;
    let hover_pos = resp.hover_pos();
    for w in 0..weeks {
        let monday = start + w * 7;
        let x = rect.min.x + label_w + w as f32 * (cell + gap);
        let (_, m, _) = civil(monday);
        if m != last_month && w - last_label_col >= 3 {
            p.text(pos2(x, rect.min.y + 6.0), Align2::LEFT_CENTER, MONTHS[(m - 1) as usize], FontId::proportional(9.5), theme.muted);
            last_label_col = w;
        }
        last_month = m;
        for d in 0..7 {
            let day = monday + d;
            if day > s.today {
                continue;
            }
            let r = Rect::from_min_size(pos2(x, rect.min.y + 16.0 + d as f32 * (cell + gap)), vec2(cell, cell));
            let (v, top) = value(day);
            let color = top.map_or(theme.accent, |m| model_color(m, theme));
            p.rect_filled(r, 3.0, shade(level(v), color));
            if day == s.today {
                p.rect_stroke(r.expand(1.0), 3.5, Stroke::new(1.0_f32, theme.text.gamma_multiply(0.6)));
            }
            if hover_pos.is_some_and(|hp| r.expand(gap / 2.0).contains(hp)) {
                hovered = Some(day);
                p.rect_stroke(r.expand(1.0), 3.5, Stroke::new(1.5_f32, theme.text));
            }
        }
    }
    if let Some(day) = hovered {
        resp.on_hover_ui_at_pointer(|ui| {
            ui.label(RichText::new(day_label(day)).strong());
            match s.daily.get(&day) {
                Some(models) if !models.is_empty() => {
                    let mut v: Vec<_> = models.iter().collect();
                    v.sort_by(|a, b| metric_of(b.1, *metric).cmp(&metric_of(a.1, *metric)));
                    for (m, st) in v {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 3.5, model_color(m, theme));
                            ui.label(model_name(m));
                            ui.label(RichText::new(format!("{} out · {} total", tokens(st.output), tokens(st.total))).color(theme.muted));
                        });
                    }
                }
                _ => {
                    ui.label(RichText::new("No usage").color(theme.muted));
                }
            }
        });
    }

    // Legend + stats
    let days: Vec<(i64, u64)> = (start..=s.today).map(|d| (d, value(d).0)).collect();
    let active = days.iter().filter(|d| d.1 > 0).count();
    let busiest = days.iter().max_by_key(|d| d.1).filter(|d| d.1 > 0);
    // Like GitHub: today not used yet doesn't break the streak.
    let mut streak = 0;
    let mut it = days.iter().rev().peekable();
    if it.peek().is_some_and(|d| d.1 == 0) {
        it.next();
    }
    for (_, v) in it {
        if *v == 0 {
            break;
        }
        streak += 1;
    }
    // Model filter: quiet text toggles under the grid.
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        if text_toggle(ui, theme, "All", None, state.heat_model.is_none(), Id::new("s-term-heat-all")).clicked() {
            state.heat_model = None;
        }
        for (m, _) in &totals {
            let on = state.heat_model.as_deref() == Some(m.as_str());
            if text_toggle(ui, theme, &model_name(m), Some(model_color(m, theme)), on, Id::new(("s-term-heat-m", m))).clicked() {
                state.heat_model = if on { None } else { Some(m.clone()) };
            }
        }
    });
    // One line of stats instead of a block.
    let total: u64 = days.iter().map(|d| d.1).sum();
    let mut parts = vec![format!("{} in {weeks} weeks", tokens(total)), format!("{active} active days")];
    if let Some((d, _)) = busiest {
        let (_, m, dd) = civil(*d);
        parts.push(format!("busiest {} {dd}", MONTHS[(m - 1) as usize]));
    }
    if streak > 0 {
        parts.push(format!("{streak}-day streak"));
    }
    ui.add_space(4.0);
    ui.label(RichText::new(parts.join("  ·  ")).size(10.5).color(theme.muted));
}

// ------------------------------------------------------------------------------------------ format

pub fn tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => trim(format!("{:.1}k", n as f64 / 1e3)),
        1_000_000..=999_999_999 => trim(format!("{:.1}M", n as f64 / 1e6)),
        _ => trim(format!("{:.2}B", n as f64 / 1e9)),
    }
}

fn trim(s: String) -> String {
    s.replace(".0k", "k").replace(".0M", "M")
}

fn duration(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d {}h", s / 86400, (s % 86400) / 3600),
    }
}

fn ago(now: i64, t: i64) -> String {
    if now - t < 10 {
        "now".into()
    } else {
        format!("{} ago", duration(now - t))
    }
}

/// `claude-opus-5-5` -> `Opus 5.5`, `gemini-4-neon-high` -> `Gemini 4 Neon · high`.
pub fn model_name(id: &str) -> String {
    let id = id.split('[').next().unwrap_or(id);
    if let Some(rest) = id.strip_prefix("claude-") {
        let mut parts = rest.split('-').filter(|p| !(p.len() == 8 && p.chars().all(|c| c.is_ascii_digit())));
        let family = parts.next().unwrap_or("");
        let version: Vec<&str> = parts.collect();
        return format!("{}{} {}", family[..1.min(family.len())].to_uppercase(), family.get(1..).unwrap_or(""), version.join(".")).trim().to_string();
    }
    if let Some(rest) = id.strip_prefix("gemini-") {
        let parts: Vec<&str> = rest.split('-').collect();
        let tiers = ["low", "medium", "high", "xhigh", "max"];
        let (body, tier) = match parts.last() {
            Some(t) if tiers.contains(t) => (&parts[..parts.len() - 1], Some(*t)),
            _ => (&parts[..], None),
        };
        let body: Vec<String> = body.iter().map(|w| format!("{}{}", w[..1.min(w.len())].to_uppercase(), w.get(1..).unwrap_or(""))).collect();
        let mut s = format!("Gemini {}", body.join(" "));
        if let Some(t) = tier {
            s.push_str(&format!(" · {t}"));
        }
        return s;
    }
    id.to_string()
}

pub fn model_color(id: &str, theme: &Theme) -> Color32 {
    let l = id.to_lowercase();
    if l.contains("opus") {
        CLAUDE_ORANGE
    } else if l.contains("sonnet") {
        c32(theme.palette[5])
    } else if l.contains("haiku") {
        c32(theme.palette[2])
    } else if l.contains("fable") {
        c32(theme.palette[3])
    } else if l.contains("gemini") {
        let h = l.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        [GEMINI_BLUE, GEMINI_PURPLE, c32(theme.palette[6]), GEMINI_PINK][(h % 4) as usize]
    } else {
        c32(theme.palette[4])
    }
}

fn level_color(pct: f32, theme: &Theme) -> Color32 {
    if pct >= 90.0 {
        c32(theme.palette[1])
    } else if pct >= 75.0 {
        c32(theme.palette[3])
    } else {
        c32(theme.palette[2])
    }
}

fn context_color(pct: f32, theme: &Theme) -> Color32 {
    if pct >= 75.0 {
        c32(theme.palette[1])
    } else if pct >= 50.0 {
        c32(theme.palette[3])
    } else {
        theme.accent2
    }
}

// ------------------------------------------------------------------------------------------- icons

/// Claude-style spark: rays of alternating length around a center.
pub fn claude_icon(p: &Painter, c: Pos2, r: f32) {
    let n = 12;
    for i in 0..n {
        let a = i as f32 / n as f32 * TAU + 0.13;
        let len = if i % 2 == 0 { r } else { r * 0.72 };
        let dir = vec2(a.cos(), a.sin());
        p.line_segment([c + dir * r * 0.18, c + dir * len], Stroke::new(r * 0.24, CLAUDE_ORANGE));
    }
    p.circle_filled(c, r * 0.24, CLAUDE_ORANGE);
}

/// Four-point sparkle with a blue → purple → pink gradient.
pub fn gemini_icon(p: &Painter, c: Pos2, r: f32) {
    let mut mesh = Mesh::default();
    let color_at = |pt: Pos2| {
        let t = ((pt.x - c.x) + (pt.y - c.y) + 2.0 * r) / (4.0 * r);
        if t < 0.5 {
            lerp_color(GEMINI_BLUE, GEMINI_PURPLE, t * 2.0)
        } else {
            lerp_color(GEMINI_PURPLE, GEMINI_PINK, (t - 0.5) * 2.0)
        }
    };
    mesh.colored_vertex(c, color_at(c));
    let steps = 14;
    let mut ring = Vec::new();
    for k in 0..4 {
        let a0 = k as f32 * PI / 2.0 - PI / 2.0;
        let a1 = a0 + PI / 2.0;
        let tip0 = c + vec2(a0.cos(), a0.sin()) * r;
        let tip1 = c + vec2(a1.cos(), a1.sin()) * r;
        let am = (a0 + a1) / 2.0;
        let ctrl = c + vec2(am.cos(), am.sin()) * r * 0.08;
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let q = tip0.to_vec2() * (1.0 - t) * (1.0 - t) + ctrl.to_vec2() * 2.0 * t * (1.0 - t) + tip1.to_vec2() * t * t;
            ring.push(q.to_pos2());
        }
    }
    for pt in &ring {
        mesh.colored_vertex(*pt, color_at(*pt));
    }
    let n = ring.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(Shape::mesh(mesh));
}

fn ring(p: &Painter, c: Pos2, r: f32, frac: f32, color: Color32, track: Color32, width: f32) {
    p.circle_stroke(c, r, Stroke::new(width, track));
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let n = (48.0 * frac).ceil().max(2.0) as usize;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = -PI / 2.0 + TAU * frac * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(width, color)));
}

// ------------------------------------------------------------------------------------------ pieces

/// A section heading: small muted caps on the left, optional controls on the right.
fn section(ui: &mut egui::Ui, theme: &Theme, title: &str, right: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(10.5).strong().color(theme.muted.gamma_multiply(0.9)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    ui.add_space(8.0);
}

/// A borderless toggle: muted text, full-strength when on, with an optional color dot.
fn text_toggle(ui: &mut egui::Ui, theme: &Theme, label: &str, dot: Option<Color32>, on: bool, id: Id) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(11.0), Color32::WHITE);
    let dw = if dot.is_some() { 11.0 } else { 0.0 };
    let (r, resp) = ui.allocate_exact_size(vec2(galley.size().x + dw + 12.0, 20.0), CLICK);
    let t = ui.ctx().animate_bool_with_time(id, on, 0.15);
    let h = ui.ctx().animate_bool_with_time(id.with("h"), resp.hovered(), 0.1);
    let p = ui.painter();
    if t > 0.0 || h > 0.0 {
        p.rect_filled(r, 10.0, theme.header.gamma_multiply(t.max(h * 0.6)));
    }
    let mut x = r.min.x + 6.0;
    if let Some(c) = dot {
        p.circle_filled(pos2(x + 3.0, r.center().y), 3.0, c.gamma_multiply(0.45 + 0.55 * t.max(h)));
        x += dw;
    }
    let col = lerp_color(theme.muted.gamma_multiply(0.85), theme.text, t.max(h * 0.5));
    p.galley_with_override_text_color(pos2(x, r.center().y - galley.size().y / 2.0), galley, col);
    resp
}

fn thin_bar(p: &Painter, r: Rect, frac: f32, color: Color32, theme: &Theme) {
    p.rect_filled(r, r.height() / 2.0, theme.header);
    let w = r.width() * frac.clamp(0.0, 1.0);
    if w > 0.0 {
        p.rect_filled(Rect::from_min_size(r.min, vec2(w.max(r.height()), r.height())), r.height() / 2.0, color);
    }
}

fn pct_of(live: &Live) -> Option<f32> {
    live.context_pct.or_else(|| live.context_size.map(|s| live.context_tokens as f32 / s as f32 * 100.0))
}

/// A live session: a context ring, the session name with its tool's icon, and one line of detail.
#[allow(clippy::too_many_arguments)]
fn live_row(ui: &mut egui::Ui, theme: &Theme, icon: fn(&Painter, Pos2, f32), live: &Live, now: i64, working: bool, accent: Color32) {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let p = ui.painter();
    let c = pos2(r.min.x + 18.0, r.center().y);
    let pct = pct_of(live);
    match pct {
        Some(v) => {
            ring(p, c, 15.0, v / 100.0, context_color(v, theme), theme.header, 3.0);
            p.text(c, Align2::CENTER_CENTER, format!("{v:.0}"), FontId::proportional(10.5), theme.text);
        }
        None => {
            ring(p, c, 15.0, 0.0, accent, theme.header, 3.0);
            icon(p, c, 6.0);
        }
    }
    let x = r.min.x + 44.0;
    // Status on the right: a pulsing dot while the agent works.
    let dot = if working {
        let t = ui.input(|i| i.time) as f32;
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
        c32(theme.palette[2]).gamma_multiply(0.45 + 0.55 * (t * 3.0).sin().abs())
    } else {
        theme.muted.gamma_multiply(0.6)
    };
    p.circle_filled(pos2(r.max.x - 4.0, r.min.y + 12.0), 3.0, dot);
    let clip = p.with_clip_rect(Rect::from_min_max(pos2(x, r.min.y), pos2(r.max.x - 14.0, r.max.y)));
    // The tool's icon sits in the ring when there's no percentage; otherwise it leads the name.
    let tx = if pct.is_some() {
        icon(&clip, pos2(x + 5.0, r.min.y + 12.0), 4.5);
        x + 15.0
    } else {
        x
    };
    clip.text(pos2(tx, r.min.y + 12.0), Align2::LEFT_CENTER, &live.label, FontId::proportional(12.5), theme.text);
    let mut ctx_s = tokens(live.context_tokens);
    if let Some(sz) = live.context_size {
        ctx_s.push_str(&format!(" / {}", tokens(sz)));
    }
    let detail = format!("{}  ·  {ctx_s}  ·  {}", model_name(&live.model), ago(now, live.last_ts));
    clip.text(pos2(x, r.min.y + 29.0), Align2::LEFT_CENTER, detail, FontId::proportional(11.0), theme.muted);
    let here = if live.here { "" } else { "\nMost recent session; not in this pane's folder." };
    resp.on_hover_text(format!("{} this session{here}", tokens(live.totals.iter().map(|m| m.total()).sum())));
    if let Some(v) = pct.filter(|v| *v >= 50.0) {
        let hint = if v >= 75.0 { "Context is nearly full: start a new session or /compact" } else { "Good moment to /compact" };
        ui.horizontal(|ui| {
            ui.add_space(44.0);
            ui.label(RichText::new(hint).size(10.5).color(context_color(v, theme)));
        });
    }
}

/// A tool with no open session: its icon and when it was last used.
fn idle_row(ui: &mut egui::Ui, theme: &Theme, icon: fn(&Painter, Pos2, f32), name: &str, last: Option<i64>, now: i64) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let p = ui.painter();
    icon(p, pos2(r.min.x + 18.0, r.center().y), 5.0);
    let when = last.map_or("no activity yet".to_string(), |t| format!("idle · {}", ago(now, t)));
    p.text(pos2(r.min.x + 44.0, r.center().y), Align2::LEFT_CENTER, format!("{name}  ·  {when}"), FontId::proportional(11.5), theme.muted);
}

fn limit_row(ui: &mut egui::Ui, theme: &Theme, label: &str, l: &Limit, now: i64) {
    let color = level_color(l.pct, theme);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let p = ui.painter();
    let y = r.min.y + 8.0;
    let g = p.layout_no_wrap(label.to_string(), FontId::proportional(11.5), theme.muted);
    let lw = g.size().x;
    p.galley(pos2(r.min.x, y - g.size().y / 2.0), g, theme.muted);
    p.text(pos2(r.min.x + lw + 8.0, y), Align2::LEFT_CENTER, format!("{:.0}%", l.pct), FontId::proportional(12.0), color);
    if let Some(t) = l.resets_at {
        p.text(pos2(r.max.x, y), Align2::RIGHT_CENTER, format!("resets in {}", duration(t - now)), FontId::proportional(10.5), theme.muted);
    }
    thin_bar(p, Rect::from_min_max(pos2(r.min.x, r.max.y - 6.0), pos2(r.max.x, r.max.y - 2.0)), l.pct / 100.0, color, theme);
}

/// A model: dot, name, total, and a thin bar relative to the busiest model. Hover for the breakdown.
fn model_row(ui: &mut egui::Ui, theme: &Theme, m: &ModelTotal, max_total: u64) {
    let color = model_color(&m.model, theme);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let p = ui.painter();
    let y = r.min.y + 8.0;
    p.circle_filled(pos2(r.min.x + 4.0, y), 3.0, color);
    p.text(pos2(r.min.x + 14.0, y), Align2::LEFT_CENTER, model_name(&m.model), FontId::proportional(12.0), theme.text);
    p.text(pos2(r.max.x, y), Align2::RIGHT_CENTER, tokens(m.total()), FontId::proportional(11.5), theme.muted);
    let track = Rect::from_min_max(pos2(r.min.x + 14.0, r.max.y - 6.0), pos2(r.max.x, r.max.y - 3.0));
    thin_bar(p, track, m.total() as f32 / max_total.max(1) as f32, color.gamma_multiply(0.85), theme);
    resp.on_hover_ui(|ui| {
        ui.label(RichText::new(model_name(&m.model)).strong());
        ui.label(RichText::new(&*m.model).size(11.0).color(theme.muted));
        egui::Grid::new("tt").num_columns(2).show(ui, |ui| {
            for (k, v) in [("input", m.input), ("cache write", m.cache_write), ("cache read", m.cache_read), ("output", m.output)] {
                ui.label(k);
                ui.label(RichText::new(tokens(v)));
                ui.end_row();
            }
            ui.label("requests");
            ui.label(RichText::new(m.requests.to_string()));
            ui.end_row();
        });
    });
}

fn sparkline(ui: &mut egui::Ui, theme: &Theme, hourly: &[u64; 24], color: Color32) {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let p = ui.painter();
    let max = *hourly.iter().max().unwrap_or(&0);
    let w = r.width() / 24.0;
    let hover = resp.hover_pos().map(|pos| (((pos.x - r.min.x) / w) as usize).min(23));
    p.line_segment([pos2(r.min.x, r.max.y + 0.5), pos2(r.max.x, r.max.y + 0.5)], Stroke::new(1.0_f32, theme.header));
    for (i, &v) in hourly.iter().enumerate() {
        if v == 0 || max == 0 {
            continue;
        }
        let h = (v as f32 / max as f32 * r.height()).max(2.0);
        let x = r.min.x + i as f32 * w;
        let c = if hover == Some(i) { theme.text } else { color.gamma_multiply(0.35 + 0.65 * v as f32 / max as f32) };
        p.rect_filled(Rect::from_min_max(pos2(x + 1.5, r.max.y - h), pos2(x + w - 1.5, r.max.y)), 1.5, c);
    }
    if let Some(i) = hover {
        let hours_ago = 23 - i as i64;
        let when = if hours_ago == 0 { "this hour".to_string() } else { format!("{hours_ago}h ago") };
        resp.on_hover_text(format!("{when}: {} output tokens", tokens(hourly[i])));
    }
}

/// Compact "5h · Today · 7d" picker with the sliding selection pill.
fn window_picker(ui: &mut egui::Ui, theme: &Theme, state: &mut UsageUi) {
    let opts = [(Window::Week, "7d"), (Window::Today, "Today"), (Window::FiveHours, "5h")];
    // Laid out right-to-left by the section header, so allocate the whole strip at once.
    let (rect, _) = ui.allocate_exact_size(vec2(132.0, 22.0), Sense::hover());
    let w = rect.width() / opts.len() as f32;
    let order = [2usize, 1, 0]; // show 5h · Today · 7d left to right
    let slot = |i: usize| Rect::from_min_size(pos2(rect.min.x + w * order[i] as f32, rect.min.y), vec2(w, rect.height()));
    if let Some(i) = opts.iter().position(|(win, _)| state.window == *win) {
        let shown = crate::glass::slide(ui.ctx(), Id::new("s-term-usage-win-pick"), slot(i), true);
        crate::glass::pick(ui.painter(), shown, theme);
    }
    for (i, (win, label)) in opts.iter().enumerate() {
        let r = slot(i);
        let resp = ui.interact(r, Id::new(("s-term-usage-win", i)), CLICK);
        let on = ui.ctx().animate_bool_with_time(Id::new(("s-term-usage-win-on", i)), state.window == *win, 0.15);
        let col = lerp_color(lerp_color(theme.muted, theme.text, if resp.hovered() { 0.6 } else { 0.0 }), theme.pick_ink, on);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *label, FontId::proportional(11.0), col);
        if resp.clicked() {
            state.window = *win;
        }
    }
}

pub fn card(ui: &mut egui::Ui, theme: &Theme, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none().fill(theme.elevated).stroke(Stroke::new(1.0_f32, theme.border)).rounding(12.0).inner_margin(12.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
    ui.add_space(8.0);
}

// ------------------------------------------------------------------------------------------- public

/// The AI tab: one flat page in four sections (now, plan limits, usage, activity). No cards.
pub fn tab(ui: &mut egui::Ui, theme: &Theme, snap: Option<&Snapshot>, state: &mut UsageUi, cfg: &mut Config) {
    let Some(s) = snap else {
        ui.label(RichText::new("Reading usage…").color(theme.muted));
        return;
    };
    let claude = s.claude.installed && cfg.show_claude;
    let agy = s.agy.installed && cfg.show_antigravity;
    if !claude && !agy {
        ui.add_space(8.0);
        ui.label(RichText::new("No Claude Code or Antigravity CLI data found.").color(theme.muted));
        return;
    }
    let tools: Vec<(&ToolStats, &str, fn(&Painter, Pos2, f32), Color32)> = [
        (claude, &s.claude, "Claude Code", claude_icon as fn(&Painter, Pos2, f32), CLAUDE_ORANGE),
        (agy, &s.agy, "Antigravity", gemini_icon as fn(&Painter, Pos2, f32), GEMINI_BLUE),
    ]
    .into_iter()
    .filter(|t| t.0)
    .map(|(_, st, n, i, a)| (st, n, i, a))
    .collect();

    // ---- now
    ui.add_space(-10.0);
    section(ui, theme, "NOW", |_| {});
    for (st, name, icon, accent) in &tools {
        match &st.live {
            Some(l) => live_row(ui, theme, *icon, l, s.now, s.now - l.last_ts < 120, *accent),
            None => idle_row(ui, theme, *icon, name, st.last_active, s.now),
        }
        ui.add_space(2.0);
    }

    // ---- plan limits
    if claude {
        match &s.limits {
            Some(l) if l.five_hour.is_some() || l.seven_day.is_some() => {
                section(ui, theme, "PLAN LIMITS", |ui| {
                    ui.label(RichText::new(format!("updated {}", ago(s.now, l.updated))).size(10.0).color(theme.muted.gamma_multiply(0.8)));
                });
                if let Some(w) = &l.five_hour {
                    limit_row(ui, theme, "5-hour", w, s.now);
                }
                if let Some(w) = &l.seven_day {
                    ui.add_space(4.0);
                    limit_row(ui, theme, "Weekly", w, s.now);
                }
            }
            _ => {
                section(ui, theme, "PLAN LIMITS", |_| {});
                let msg = if s.statusline_seen {
                    "Plan limits appear after Claude Code's next reply."
                } else {
                    "Set Claude Code's statusLine to `s-term statusline` to see your 5-hour and weekly limits (see README)."
                };
                ui.label(RichText::new(msg).size(11.0).color(theme.muted));
            }
        }
    }

    // ---- usage: one total, one list of models across tools, one 24h chart
    section(ui, theme, "USAGE", |ui| window_picker(ui, theme, state));
    let mut models: Vec<&ModelTotal> = tools.iter().flat_map(|(st, ..)| st.windows.get(&state.window).map(|v| v.iter()).into_iter().flatten()).collect();
    models.sort_by_key(|m| std::cmp::Reverse(m.total()));
    let total: u64 = models.iter().map(|m| m.total()).sum();
    let output: u64 = models.iter().map(|m| m.output).sum();
    ui.horizontal(|ui| {
        ui.label(RichText::new(tokens(total)).size(22.0).strong().color(theme.text));
        ui.label(RichText::new("tokens").size(11.0).color(theme.muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{} output", tokens(output))).size(11.0).color(theme.muted));
        });
    });
    ui.add_space(6.0);
    if models.is_empty() {
        ui.label(RichText::new("No usage in this period.").size(11.5).color(theme.muted));
    }
    let max = models.first().map_or(1, |m| m.total());
    for m in &models {
        model_row(ui, theme, m, max);
    }
    let mut hourly = [0u64; 24];
    for (st, ..) in &tools {
        for (h, v) in hourly.iter_mut().zip(st.hourly) {
            *h += v;
        }
    }
    let sessions: usize = tools.iter().map(|(st, ..)| st.sessions_today).sum();
    ui.add_space(10.0);
    sparkline(ui, theme, &hourly, theme.accent);
    ui.add_space(3.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Output, last 24 hours").size(10.5).color(theme.muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{sessions} session{} today", if sessions == 1 { "" } else { "s" })).size(10.5).color(theme.muted));
        });
    });

    // ---- activity
    heatmap(ui, theme, s, state, &mut cfg.heat_metric);
    ui.add_space(12.0);
}

/// Status-bar meter for whichever AI CLI is running: Claude Code (this pane's context, the 5-hour and
/// weekly plan limits) or, when only it is running, Antigravity (context and recent token use).
/// Hidden when neither is running.
pub fn status_meter(ui: &mut egui::Ui, theme: &Theme, s: &Snapshot, show_agy: bool, height: f32) -> Option<egui::Response> {
    // (label, value text, bar fill %, color)
    let mut segs: Vec<(&str, String, Option<f32>, Color32)> = Vec::new();
    let mut tip: Vec<String> = Vec::new();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let icon: fn(&Painter, Pos2, f32);
    if s.claude.running {
        icon = claude_icon;
        if let Some(l) = s.claude.live.as_ref().filter(|l| l.here) {
            let pct = l.context_pct.or_else(|| l.context_size.map(|sz| l.context_tokens as f32 / sz as f32 * 100.0));
            let used = match l.context_size {
                Some(sz) => format!("{} / {}", tokens(l.context_tokens), tokens(sz)),
                None => tokens(l.context_tokens),
            };
            if let Some(p) = pct {
                segs.push(("Context", format!("{p:.0}%"), Some(p), context_color(p, theme)));
                tip.push(format!("Context window: {p:.0}% used ({used})  ·  {}", model_name(&l.model)));
            } else {
                tip.push(format!("Context window: {used} tokens  ·  {}", model_name(&l.model)));
            }
        }
        let lim = s.limits.as_ref();
        let resets = |l: &Limit| l.resets_at.filter(|t| *t > now).map_or(String::new(), |t| format!("  ·  resets in {}", duration(t - now)));
        if let Some(f) = lim.and_then(|l| l.five_hour) {
            segs.push(("5h", format!("{:.0}%", f.pct), Some(f.pct), level_color(f.pct, theme)));
            tip.push(format!("5-hour limit: {:.0}% used{}", f.pct, resets(&f)));
        }
        if let Some(w) = lim.and_then(|l| l.seven_day) {
            segs.push(("Week", format!("{:.0}%", w.pct), Some(w.pct), level_color(w.pct, theme)));
            tip.push(format!("Weekly limit: {:.0}% used{}", w.pct, resets(&w)));
        }
    } else if s.agy.running && show_agy {
        icon = gemini_icon;
        if let Some(l) = s.agy.live.as_ref().filter(|l| l.here) {
            segs.push(("Context", tokens(l.context_tokens), None, GEMINI_BLUE));
            tip.push(format!("Context: {} tokens  ·  {}  ·  {}", tokens(l.context_tokens), model_name(&l.model), l.label));
        }
        let sum = |w: Window| s.agy.windows.get(&w).map_or(0, |v| v.iter().map(|m| m.total()).sum::<u64>());
        let (h5, wk) = (sum(Window::FiveHours), sum(Window::Week));
        segs.push(("5h", tokens(h5), None, GEMINI_BLUE));
        segs.push(("Week", tokens(wk), None, GEMINI_BLUE));
        tip.push(format!("Antigravity tokens: {} in the last 5 hours  ·  {} in the last 7 days", tokens(h5), tokens(wk)));
    } else {
        return None;
    }
    if segs.is_empty() {
        return None;
    }

    const BAR_W: f32 = 26.0;
    const GAP: f32 = 5.0;
    const SEP: f32 = 13.0;
    let label_font = FontId::proportional(10.5);
    let val_font = FontId::proportional(11.5);
    let laid: Vec<_> = segs
        .iter()
        .map(|(name, val, _, _)| {
            let n = ui.painter().layout_no_wrap(name.to_string(), label_font.clone(), theme.muted);
            let v = ui.painter().layout_no_wrap(val.clone(), val_font.clone(), theme.text);
            (n, v)
        })
        .collect();
    let content: f32 = laid.iter().zip(&segs).map(|((n, v), seg)| n.size().x + GAP + if seg.2.is_some() { BAR_W + GAP } else { 0.0 } + v.size().x).sum::<f32>() + SEP * (laid.len() - 1) as f32;
    let (r, resp) = ui.allocate_exact_size(vec2(22.0 + content + 10.0, height), CLICK);
    let hover = ui.ctx().animate_bool_with_time(ui.id().with("s-term-status-meter"), resp.hovered(), 0.1);
    let p = ui.painter();
    p.rect_filled(r, r.height() / 2.0, lerp_color(theme.header.gamma_multiply(0.6), theme.header, hover));
    icon(p, pos2(r.min.x + 12.0, r.center().y), 4.6);
    let mut x = r.min.x + 22.0;
    let cy = r.center().y;
    for (i, ((n, v), (_, _, pct, col))) in laid.into_iter().zip(&segs).enumerate() {
        if i > 0 {
            let sx = x - SEP / 2.0;
            p.line_segment([pos2(sx, cy - 5.0), pos2(sx, cy + 5.0)], Stroke::new(1.0_f32, theme.border));
        }
        p.galley(pos2(x, cy - n.size().y / 2.0), n.clone(), theme.muted);
        x += n.size().x + GAP;
        if let Some(pct) = pct {
            let bar = Rect::from_min_size(pos2(x, cy - 2.0), vec2(BAR_W, 4.0));
            p.rect_filled(bar, 2.0, theme.bar);
            let f = (pct / 100.0).clamp(0.0, 1.0);
            if f > 0.0 {
                p.rect_filled(Rect::from_min_size(bar.min, vec2((BAR_W * f).max(4.0), 4.0)), 2.0, *col);
            }
            x += BAR_W + GAP;
        }
        let vc = if pct.is_some_and(|p| p >= 75.0) { *col } else { theme.text };
        p.galley(pos2(x, cy - v.size().y / 2.0), v.clone(), vc);
        x += v.size().x + SEP;
    }
    tip.push("Click to open AI usage".into());
    Some(resp.on_hover_text(tip.join("\n")))
}

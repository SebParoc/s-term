//! Terminal panes: animated layout, mouse handling and painting.

use alacritty_terminal::grid::{Dimensions as _, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, FontId, Id, Pos2, Rect, Rounding, Stroke, Vec2};

use super::{STerm, CLICK_DRAG, DRAG};
use crate::config::{CursorStyle, PaneHeaders};
use crate::icons::{self, Icon};
use crate::input;
use crate::layout::{Divider, Orientation};
use crate::pane::{GridSize, Press};
use crate::render;
use crate::theme::{lerp_color, lerp_rect};

const HEADER_H: f32 = 40.0;
use crate::glass::PANE_GAP as GAP;
const PAD: f32 = 6.0;
use crate::glass::PANE_ROUND as ROUND;
/// Pane outline width in points: under 1px, so it renders as a fainter hairline.
/// Exponential smoothing rates (1/s): higher = snappier.
const LAYOUT_SPEED: f32 = 16.0;
const CURSOR_SPEED: f32 = 38.0;
/// The cursor's trailing edge, slower than the head so moves leave a short smear.
const CURSOR_TAIL_SPEED: f32 = 18.0;
const TAB_FADE_SECS: f32 = 0.15;
/// Lines per second a selection dragged past the top or bottom of the text scrolls: the minimum, how much more for
/// each line's height further out, and the cap.
const EDGE_SCROLL_MIN: f32 = 10.0;
const EDGE_SCROLL_PER_CELL: f32 = 6.0;
const EDGE_SCROLL_MAX: f32 = 200.0;

fn grid_point(pos: Pos2, origin: Pos2, cell: Vec2, size: GridSize, offset: usize) -> (Point, Side) {
    let x = ((pos.x - origin.x) / cell.x).max(0.0);
    let y = ((pos.y - origin.y) / cell.y).max(0.0);
    let col = (x as usize).min(size.cols.saturating_sub(1));
    let row = (y as usize).min(size.rows.saturating_sub(1));
    let side = if x.fract() > 0.5 { Side::Right } else { Side::Left };
    (Point::new(Line(row as i32 - offset as i32), Column(col)), side)
}

/// The (column, row) on screen under `pos`, clamped to the grid so a drag past the pane's edge stays on it.
fn cell_at(pos: Pos2, origin: Pos2, cell: Vec2, size: GridSize) -> (usize, usize) {
    let col = (((pos.x - origin.x) / cell.x).max(0.0) as usize).min(size.cols.saturating_sub(1));
    let row = (((pos.y - origin.y) / cell.y).max(0.0) as usize).min(size.rows.saturating_sub(1));
    (col, row)
}

/// Splits a pane rect into (header, body, terminal text area).
fn pane_parts(r: Rect, header_h: f32) -> (Rect, Rect, Rect) {
    let header = Rect::from_min_size(r.min, vec2(r.width(), header_h));
    let body = Rect::from_min_max(pos2(r.min.x, header.max.y), r.max);
    (header, body, body.shrink2(vec2(PAD + 2.0, PAD)))
}

impl STerm {
    pub(super) fn panes_ui(&mut self, ui: &mut egui::Ui) {
        let full = ui.max_rect();
        self.central = full;
        let area = full.shrink(GAP);
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let Some(tab) = self.tabs.get(self.active) else { return };
        let anim = self.config.animations;

        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 1.0 / 30.0);
        let k = if anim { 1.0 - (-dt * LAYOUT_SPEED).exp() } else { 1.0 };
        let glide = self.config.cursor_glide && anim;
        let cursor_k = if glide { 1.0 - (-dt * CURSOR_SPEED).exp() } else { 1.0 };
        let cursor_tail_k = if glide { 1.0 - (-dt * CURSOR_TAIL_SPEED).exp() } else { 1.0 };
        let time = ctx.input(|i| i.time);
        let blink = self.config.cursor_blink && ctx.input(|i| i.focused);
        let mut next_frame = f32::INFINITY;

        let mut targets = Vec::new();
        let mut dividers: Vec<Divider> = Vec::new();
        match tab.zoomed {
            Some(z) if self.panes.contains_key(&z) => targets.push((z, area)),
            _ => tab.root.layout(area, GAP, &mut targets, &mut dividers, &mut Vec::new()),
        }
        let focus = tab.focus;
        let broadcast = tab.broadcast;
        let multi = targets.len() > 1;
        let header_h = match self.config.pane_headers {
            PaneHeaders::Always => HEADER_H,
            PaneHeaders::Splits if multi => HEADER_H,
            _ => 0.0,
        };

        // Window resizes, divider drags and tab switches follow exactly; split/close/zoom glide.
        let switched = self.active != self.last_active;
        let snap = area != self.last_area || self.dragging_divider || switched;
        if switched && anim {
            self.tab_fade = 1.0;
        }
        self.last_area = area;
        self.last_active = self.active;

        let mut animating = false;
        let mut shown = Vec::with_capacity(targets.len());
        for &(id, target) in &targets {
            let start = match self.spawn_from.remove(&id) {
                Some(Orientation::SideBySide) => Rect::from_min_max(pos2(target.max.x, target.min.y), target.max),
                Some(Orientation::Stacked) => Rect::from_min_max(pos2(target.min.x, target.max.y), target.max),
                None => target,
            };
            let cur = self.anim_rects.get(&id).copied().unwrap_or(start);
            let mut r = if snap { target } else { lerp_rect(cur, target, k) };
            if (r.min - target.min).length() + (r.max - target.max).length() < 0.5 {
                r = target;
            } else {
                animating = true;
            }
            self.anim_rects.insert(id, r);
            shown.push((id, target, r));
        }
        let panes = &self.panes;
        self.anim_rects.retain(|id, _| panes.contains_key(id));

        // A file is being dragged from the side panel (or the desktop): highlight the pane under the pointer.
        let os_drag = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let drop_pos = if self.sidebar.dragging().is_some() || os_drag { ctx.pointer_latest_pos() } else { None };
        // A single folder dropped on a pane at an empty shell prompt cds there (see `drop_paths`).
        let drag_dir = match self.sidebar.dragging() {
            Some(p) => p.is_dir(),
            None => ctx.input(|i| matches!(&i.raw.hovered_files[..], [f] if f.path.as_ref().is_some_and(|p| p.is_dir()))),
        };
        let theme = &self.theme;
        let cell = self.cell;
        let painter = ui.painter().clone();

        let mut new_focus = None;
        // egui turns Shift+wheel into horizontal scrolling; here it's still vertical (it bypasses mouse reporting).
        let scroll_y = ui.input(|i| if i.modifiers.shift { i.smooth_scroll_delta.x } else { i.smooth_scroll_delta.y }) * self.config.scroll_speed;
        let cursor_shape = match self.config.cursor_style {
            CursorStyle::App => None,
            CursorStyle::Block => Some(CursorShape::Block),
            CursorStyle::Beam => Some(CursorShape::Beam),
            CursorStyle::Underline => Some(CursorShape::Underline),
        };
        let press_origin = ui.input(|i| i.pointer.press_origin());
        let snap_px = |p: Pos2| pos2((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp);
        let mut copy_text = None;

        for &(id, target, r) in &shown {
            let Some(pane) = self.panes.get_mut(&id) else { continue };
            let focused = id == focus;

            // The PTY is sized from the final layout, so it doesn't reflow on every animation frame.
            let (_, _, target_inner) = pane_parts(target, header_h);
            let size = GridSize {
                cols: ((target_inner.width() / cell.x).floor() as usize).max(2),
                rows: ((target_inner.height() / cell.y).floor() as usize).max(1),
            };
            pane.resize(size, cell);

            let (header, body, inner) = pane_parts(r, header_h);
            let origin = snap_px(inner.min);

            // ---- mouse
            let resp = ui.interact(r, Id::new(("s-term-pane", id)), CLICK_DRAG);
            if resp.hovered() {
                ctx.set_cursor_icon(CursorIcon::Text);
            }
            if resp.clicked() || resp.drag_started() {
                new_focus = Some(id);
            }
            {
                let mut term = pane.term.lock();
                let mode = *term.mode();
                let mods = ui.input(|i| i.modifiers);
                let sgr = mode.contains(TermMode::SGR_MOUSE);
                let app_mouse = mode.intersects(TermMode::MOUSE_MODE);
                let offset = term.grid().display_offset();
                let at = |pos| grid_point(pos, origin, cell, size, offset);
                // Bytes for the program, written once the terminal is unlocked.
                let mut out = Vec::new();
                let down = resp.is_pointer_button_down_on();

                // Who gets a press is decided when the button goes down. A program that asked for mouse events
                // (Claude Code's fullscreen view, vim, htop...) gets it and does its own selection; Shift keeps it
                // for S-term's selection, which is anchored now so scrolling before the drag can't move it.
                if down && pane.press.is_none() {
                    let pos = press_origin.or(resp.interact_pointer_pos()).unwrap_or(origin);
                    // Presses on the header only focus the pane.
                    pane.press = Some(if app_mouse && !mods.shift && body.contains(pos) {
                        let button = ui.input(|i| if i.pointer.primary_down() { 0 } else if i.pointer.middle_down() { 1 } else { 2 });
                        let (col, row) = cell_at(pos, origin, cell, size);
                        out.extend(input::mouse_report(button | input::mouse_mods(mods), col, row, sgr, true));
                        pane.mouse_cell = Some((col, row));
                        term.selection = None;
                        Press::App { button }
                    } else {
                        let (anchor, side) = at(pos);
                        Press::Select { anchor, side, started: false }
                    });
                }
                let press = pane.press;
                match press {
                    Some(Press::App { button }) => {
                        // Moves and the release are reported at the nearest cell, even off the pane's edge.
                        let pos = resp.interact_pointer_pos().or(ctx.pointer_latest_pos()).unwrap_or(origin);
                        let here = cell_at(pos, origin, cell, size);
                        if !down {
                            // Not if the program turned mouse reporting off meanwhile (or exited): the shell would get it as typing.
                            if app_mouse {
                                out.extend(input::mouse_report(button | input::mouse_mods(mods), here.0, here.1, sgr, false));
                            }
                        } else if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) && pane.mouse_cell != Some(here) {
                            out.extend(input::mouse_report((button + 32) | input::mouse_mods(mods), here.0, here.1, sgr, true));
                            pane.mouse_cell = Some(here);
                        }
                    }
                    Some(Press::Select { anchor, side, started }) => {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let (pt, side) = at(pos);
                            if resp.triple_clicked() {
                                term.selection = Some(Selection::new(SelectionType::Lines, pt, side));
                            } else if resp.double_clicked() {
                                term.selection = Some(Selection::new(SelectionType::Semantic, pt, side));
                            } else if resp.clicked() && !started {
                                term.selection = None;
                            }
                        }
                        if resp.drag_started() && !started {
                            term.selection = Some(Selection::new(SelectionType::Simple, anchor, side));
                            pane.press = Some(Press::Select { anchor, side, started: true });
                        }
                        if resp.dragged() {
                            if let (Some(pos), Some(sel)) = (resp.interact_pointer_pos(), term.selection.as_mut()) {
                                let (pt, side) = at(pos);
                                sel.update(pt, side);
                            }
                        }
                        let made = (!down && (started || resp.drag_stopped())) || resp.double_clicked() || resp.triple_clicked();
                        if self.config.copy_on_select && made {
                            copy_text = term.selection_to_string().filter(|s| !s.is_empty());
                        }
                    }
                    None => {
                        // Mode 1003 also wants moves with no button held.
                        let hover = resp.hover_pos().filter(|p| mode.contains(TermMode::MOUSE_MOTION) && !mods.shift && body.contains(*p));
                        match hover.map(|pos| cell_at(pos, origin, cell, size)) {
                            Some(here) if pane.mouse_cell != Some(here) => {
                                out.extend(input::mouse_report(35 | input::mouse_mods(mods), here.0, here.1, sgr, true));
                                pane.mouse_cell = Some(here);
                            }
                            Some(_) => {}
                            None => pane.mouse_cell = None,
                        }
                    }
                }
                if !down {
                    pane.press = None;
                }

                if resp.contains_pointer() && scroll_y != 0.0 {
                    pane.scroll_accum += scroll_y / cell.y;
                    let lines = pane.scroll_accum.trunc() as i32;
                    pane.scroll_accum -= lines as f32;
                    if lines != 0 {
                        let count = lines.unsigned_abs() as usize;
                        let selecting = matches!(press, Some(Press::Select { .. })) && down;
                        if selecting && !mode.contains(TermMode::ALT_SCREEN) {
                            // The wheel with the button held selects through the scrollback, from where it went
                            // down, even before the pointer has moved.
                            term.scroll_display(Scroll::Delta(lines));
                            if let Some(Press::Select { anchor, side, started }) = &mut pane.press {
                                if !*started {
                                    term.selection = Some(Selection::new(SelectionType::Simple, *anchor, *side));
                                    *started = true;
                                }
                            }
                            if let Some(pos) = resp.interact_pointer_pos() {
                                let (pt, side) = grid_point(pos, origin, cell, size, term.grid().display_offset());
                                if let Some(sel) = term.selection.as_mut() {
                                    sel.update(pt, side);
                                }
                            }
                        } else if selecting && term.selection.is_some() {
                            // A full-screen program has no scrollback, and moving its view would slide other text
                            // under the selection.
                        } else if app_mouse && !mods.shift {
                            // The program asked for mouse events: real wheel events, so it scrolls its own view
                            // (and, mid-drag, its own selection). Shift+wheel bypasses this.
                            let (col, row) = cell_at(resp.hover_pos().unwrap_or(origin), origin, cell, size);
                            let button = (if lines > 0 { 64 } else { 65 }) | input::mouse_mods(mods);
                            out.extend(input::mouse_report(button, col, row, sgr, true).repeat(count));
                        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !mods.shift {
                            // Full-screen apps without mouse support (less, man...) get arrow keys.
                            let app = mode.contains(TermMode::APP_CURSOR);
                            let seq: &[u8] = match (lines > 0, app) {
                                (true, true) => b"\x1bOA",
                                (true, false) => b"\x1b[A",
                                (false, true) => b"\x1bOB",
                                (false, false) => b"\x1b[B",
                            };
                            out.extend(seq.repeat(count));
                        } else {
                            term.scroll_display(Scroll::Delta(lines));
                        }
                    }
                }

                // Dragging a selection above or below the text scrolls the scrollback, faster the further out.
                let top = origin.y;
                let bottom = origin.y + size.rows as f32 * cell.y;
                let out_by = match (down, pane.press, resp.interact_pointer_pos()) {
                    (true, Some(Press::Select { started: true, .. }), Some(p)) if !mode.contains(TermMode::ALT_SCREEN) => {
                        if p.y < top { p.y - top } else if p.y > bottom { p.y - bottom } else { 0.0 }
                    }
                    _ => 0.0,
                };
                if out_by != 0.0 {
                    let rate = (EDGE_SCROLL_MIN + EDGE_SCROLL_PER_CELL * out_by.abs() / cell.y).min(EDGE_SCROLL_MAX);
                    pane.edge_scroll += rate * ui.input(|i| i.stable_dt).min(0.1);
                    let lines = pane.edge_scroll.trunc() as i32;
                    pane.edge_scroll -= lines as f32;
                    if lines != 0 {
                        term.scroll_display(Scroll::Delta(if out_by < 0.0 { lines } else { -lines }));
                        let off = term.grid().display_offset();
                        if let (Some(pos), Some(sel)) = (resp.interact_pointer_pos(), term.selection.as_mut()) {
                            let (pt, side) = grid_point(pos, origin, cell, size, off);
                            sel.update(pt, side);
                        }
                    }
                    // Keep going while the pointer stays out there without moving.
                    ctx.request_repaint();
                } else {
                    pane.edge_scroll = 0.0;
                }
                drop(term);
                if !out.is_empty() {
                    pane.write(out);
                }
            }

            // ---- paint
            let t = if multi { ctx.animate_bool_with_time(Id::new(("s-term-focus", id)), focused, 0.18) } else { 1.0 };
            if multi && t > 0.01 {
                let glow = egui::epaint::Shadow { offset: vec2(0.0, 0.0), blur: 22.0, spread: 0.0, color: theme.accent.gamma_multiply(0.16 * t) };
                painter.add(glow.as_shape(r, ROUND));
            }
            // Glass, like the other panels: shadow, a soft sheen (under the text) and edge lighting.
            crate::glass::pane_back(&painter, r, theme, t);

            if header_h > 0.0 {
                let top = Rounding { nw: ROUND, ne: ROUND, sw: 0.0, se: 0.0 };
                painter.rect_filled(header, top, lerp_color(theme.header, theme.header_focused, t * 0.7).gamma_multiply(0.85));
                painter.line_segment([pos2(header.min.x, header.max.y), pos2(header.max.x, header.max.y)], Stroke::new(1.0_f32, theme.border));
                let hp = painter.with_clip_rect(header.shrink2(vec2(8.0, 0.0)));
                let icon = if broadcast { Icon::Broadcast } else { Icon::Terminal };
                let ic = lerp_color(theme.muted, theme.accent, t);
                // A pane running Claude Code, Codex or Antigravity shows that tool's icon.
                let at = pos2(header.min.x + 19.0, header.center().y);
                if !(!broadcast).then(|| pane.ai_tool()).flatten().is_some_and(|tool| crate::usage_ui::tool_glyph(tool, &hp, at, 5.0, ic, t)) {
                    icons::draw(&hp, icon, icons::center_rect(at, 16.0), ic);
                }
                let title_color = lerp_color(lerp_color(theme.muted, theme.text, 0.55), theme.text, t);
                // The path is what matters now that the prompt is short: `user@host:` is dimmed, the path is bright.
                // Hierarchy: `user@host:` faint, parent path muted, folder name large and bright.
                let path = super::chrome::tab_title(&pane.title);
                let who = &pane.title[..pane.title.len() - path.len()];
                let split = path.trim_end_matches('/').rfind('/').map_or(0, |i| i + 1);
                let (parent, name) = path.split_at(split);
                let mut job = egui::text::LayoutJob::default();
                let fmt = |size: f32, color: Color32| egui::TextFormat { font_id: FontId::proportional(size), color, valign: egui::Align::Center, ..Default::default() };
                job.append(who, 0.0, fmt(12.0, theme.muted.gamma_multiply(0.6)));
                job.append(parent, 0.0, fmt(14.0, lerp_color(theme.muted, title_color, 0.35)));
                job.append(name, 0.0, fmt(16.0, title_color));
                let x = header.min.x + 37.0;
                let g = hp.layout_job(job);
                hp.galley(pos2(x, header.center().y - g.size().y / 2.0), g, title_color);
            }
            let term = pane.term.lock();
            let offset = term.grid().display_offset();
            if header_h > 0.0 {
                if offset > 0 {
                    painter.with_clip_rect(header).text(pos2(header.max.x - 12.0, header.center().y), Align2::RIGHT_CENTER, format!("↑{offset}"), FontId::monospace(11.0), theme.muted);
                }
            } else if offset > 0 {
                // Scrolled-back badge when there is no header.
                let badge = format!("↑ {offset}");
                let g = painter.layout_no_wrap(badge, FontId::monospace(11.0), theme.text);
                let br = Rect::from_min_size(pos2(r.max.x - g.size().x - 22.0, r.min.y + 8.0), g.size() + vec2(12.0, 6.0));
                painter.rect_filled(br, 6.0, theme.elevated);
                painter.galley(br.min + vec2(6.0, 3.0), g, theme.text);
            }

            let tp = painter.with_clip_rect(body.shrink(1.0));
            let frame = render::Frame { painter: &tp, origin, theme, cell, focused, faux_bold: self.faux_bold, cursor_k, cursor_tail_k, cursor_shape, time, blink };
            next_frame = next_frame.min(render::paint_terminal(&frame, &term, &mut self.glyphs, &mut pane.cursor_anim));
            drop(term);

            if multi {
                if self.config.dim_inactive {
                    let bottom = if header_h > 0.0 { Rounding { nw: 0.0, ne: 0.0, sw: ROUND, se: ROUND } } else { Rounding::same(ROUND) };
                    painter.rect_filled(body, bottom, Color32::from_black_alpha(((1.0 - t) * 70.0) as u8));
                }
            }
            crate::glass::pane_edge(&painter, r, theme, t, if multi { t } else { 0.0 });

            // ---- scrollbar: a thin floating handle over the right padding, like the side panel's.
            {
                let mut term = pane.term.lock();
                let history = term.grid().history_size();
                let alt = term.mode().contains(TermMode::ALT_SCREEN);
                let offset = term.grid().display_offset();
                let track = Rect::from_min_max(pos2(body.max.x - 9.0, inner.min.y), pos2(body.max.x - 1.0, inner.max.y));
                let bar_id = Id::new(("s-term-pane-bar", id));
                if history > 0 && !alt && track.height() > 20.0 {
                    let bar = ui.interact(track, bar_id, CLICK_DRAG);
                    let total = (history + size.rows) as f32;
                    let handle_h = (track.height() * size.rows as f32 / total).max(24.0).min(track.height());
                    let travel = track.height() - handle_h;
                    let top_frac = (history - offset) as f32 / history as f32;
                    let handle_y = track.min.y + travel * top_frac;
                    let set_top = |term: &mut alacritty_terminal::Term<_>, top: f32| {
                        let frac = if travel > 0.0 { ((top - track.min.y) / travel).clamp(0.0, 1.0) } else { 1.0 };
                        let want = history - (frac * history as f32).round() as usize;
                        let now = term.grid().display_offset();
                        term.scroll_display(Scroll::Delta(want as i32 - now as i32));
                    };
                    let grab_id = bar_id.with("grab");
                    if bar.drag_started() {
                        if let Some(p) = bar.interact_pointer_pos() {
                            // Grab the handle where it was pressed; pressing the track jumps the handle there.
                            let grab = if (handle_y..handle_y + handle_h).contains(&p.y) { p.y - handle_y } else { handle_h / 2.0 };
                            ctx.data_mut(|d| d.insert_temp(grab_id, grab));
                        }
                    }
                    if let Some(p) = bar.interact_pointer_pos() {
                        if bar.dragged() || bar.clicked() {
                            let grab = if bar.dragged() { ctx.data(|d| d.get_temp(grab_id)).unwrap_or(handle_h / 2.0) } else { handle_h / 2.0 };
                            set_top(&mut term, p.y - grab);
                            new_focus = Some(id);
                        }
                    }
                    let offset = term.grid().display_offset();
                    let handle_y = track.min.y + travel * (history - offset) as f32 / history as f32;
                    let hot = bar.hovered() || bar.dragged();
                    let show = ctx.animate_bool_with_time(bar_id.with("show"), hot || resp.hovered() || offset > 0, 0.2);
                    let wide = ctx.animate_bool_with_time(bar_id.with("wide"), hot, 0.12);
                    if show > 0.01 {
                        let w = 3.0 + 3.0 * wide;
                        let handle = Rect::from_min_max(pos2(track.max.x - 2.0 - w, handle_y), pos2(track.max.x - 2.0, handle_y + handle_h));
                        let color = theme.muted.gamma_multiply((0.35 + 0.4 * wide) * show);
                        painter.rect_filled(handle, w / 2.0, color);
                    }
                    if hot {
                        ctx.set_cursor_icon(CursorIcon::Default);
                    }
                }
            }

            let over = drop_pos.is_some_and(|p| r.contains(p));
            let drop_t = ctx.animate_bool_with_time(Id::new(("s-term-drop", id)), over, 0.12);
            if drop_t > 0.01 {
                let label = if drag_dir && pane.at_empty_prompt() { "Drop to cd here" } else { "Drop to insert path" };
                painter.rect_filled(r, ROUND, theme.accent.gamma_multiply(0.08 * drop_t));
                painter.rect_stroke(r.shrink(1.0), ROUND, Stroke::new(2.0_f32, theme.accent.gamma_multiply(drop_t)));
                let g = painter.layout_no_wrap(label.into(), FontId::proportional(13.0), theme.text);
                let br = Rect::from_center_size(pos2(r.center().x, r.max.y - 28.0), g.size() + vec2(20.0, 10.0));
                painter.rect_filled(br, 8.0, theme.elevated.gamma_multiply(drop_t));
                painter.galley(br.min + vec2(10.0, 5.0), g, theme.text.gamma_multiply(drop_t));
                animating = true;
            }
        }
        if let Some(s) = copy_text {
            ctx.output_mut(|o| o.copied_text = s);
        }

        // ---- dividers (drag to resize)
        let mut ratio_updates = Vec::new();
        let mut dragging = false;
        for (i, d) in dividers.iter().enumerate() {
            let r = ui.interact(d.rect.expand(2.0), Id::new(("s-term-div", self.active, i, d.path.clone())), DRAG);
            let hot = ctx.animate_bool_with_time(Id::new(("s-term-div-hot", self.active, i)), r.hovered() || r.dragged(), 0.12);
            if hot > 0.01 {
                let line = match d.orient {
                    Orientation::SideBySide => Rect::from_center_size(d.rect.center(), vec2(2.0, d.rect.height() * 0.3 * hot)),
                    Orientation::Stacked => Rect::from_center_size(d.rect.center(), vec2(d.rect.width() * 0.3 * hot, 2.0)),
                };
                painter.rect_filled(line, 1.0, theme.accent.gamma_multiply(hot));
            }
            if r.hovered() || r.dragged() {
                ctx.set_cursor_icon(match d.orient {
                    Orientation::SideBySide => CursorIcon::ResizeHorizontal,
                    Orientation::Stacked => CursorIcon::ResizeVertical,
                });
            }
            if r.dragged() {
                dragging = true;
                if let Some(pos) = r.interact_pointer_pos() {
                    let ratio = match d.orient {
                        Orientation::SideBySide => (pos.x - d.parent.min.x) / d.parent.width(),
                        Orientation::Stacked => (pos.y - d.parent.min.y) / d.parent.height(),
                    };
                    ratio_updates.push((d.path.clone(), ratio));
                }
            }
        }
        self.dragging_divider = dragging;

        // ---- tab switch fade-in
        if self.tab_fade > 0.0 {
            painter.rect_filled(full, 0.0, theme.bar.gamma_multiply(self.tab_fade));
            self.tab_fade = (self.tab_fade - dt / TAB_FADE_SECS).max(0.0);
            animating = true;
        }

        let tab = &mut self.tabs[self.active];
        for (path, ratio) in ratio_updates {
            tab.root.set_ratio_at(&path, ratio);
        }
        if let Some(f) = new_focus {
            tab.focus = f;
        }
        self.rects = targets;
        if animating || next_frame <= 0.0 {
            ctx.request_repaint();
        } else if next_frame.is_finite() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f32(next_frame));
        }
    }
}

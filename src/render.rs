use std::collections::HashMap;
use std::sync::Arc;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::CursorShape;
use egui::{pos2, vec2, Color32, FontFamily, FontId, Galley, Painter, Pos2, Rect, Stroke, Vec2};

use crate::fonts::{MONO_BOLD, TOFU};
use crate::pane::Listener;
use crate::theme::{c32, lerp_rect, Theme};

/// Laid-out single glyphs, reused across frames (text layout is the expensive part).
pub struct GlyphCache {
    map: HashMap<(char, bool), Arc<Galley>>,
    size: f32,
    /// Characters no installed font has, for the app to find a system font for (`fonts::Fallbacks`).
    pub missing: Vec<char>,
}

impl GlyphCache {
    pub fn new(size: f32) -> Self {
        Self { map: HashMap::new(), size, missing: Vec::new() }
    }

    pub fn reset(&mut self, size: f32) {
        self.map.clear();
        self.size = size;
    }

    fn font(&self, bold: bool) -> FontId {
        if bold {
            FontId::new(self.size, FontFamily::Name(MONO_BOLD.into()))
        } else {
            FontId::monospace(self.size)
        }
    }
}

struct GlyphReq {
    pos: Pos2,
    span: f32,
    c: char,
    combining: Option<String>,
    color: Color32,
    bold: bool,
}

/// Paints Unicode block elements (U+2580–U+259F) procedurally. Returns false for other chars.
fn paint_block(p: &Painter, cell: Rect, c: char, color: Color32) -> bool {
    // Fractions of the cell: (x0, y0, x1, y1).
    const UL: (f32, f32, f32, f32) = (0.0, 0.0, 0.5, 0.5);
    const UR: (f32, f32, f32, f32) = (0.5, 0.0, 1.0, 0.5);
    const LL: (f32, f32, f32, f32) = (0.0, 0.5, 0.5, 1.0);
    const LR: (f32, f32, f32, f32) = (0.5, 0.5, 1.0, 1.0);
    let n = c as u32;
    if (0x2800..=0x28FF).contains(&n) {
        return paint_braille(p, cell, (n - 0x2800) as u8, color);
    }
    let mut alpha = 1.0;
    let parts: &[(f32, f32, f32, f32)] = match n {
        0x2580 => &[(0.0, 0.0, 1.0, 0.5)],
        0x2581..=0x2588 => {
            let h = (n - 0x2580) as f32 / 8.0;
            return fill(p, cell, &[(0.0, 1.0 - h, 1.0, 1.0)], color);
        }
        0x2589..=0x258F => {
            let w = (0x2590 - n) as f32 / 8.0;
            return fill(p, cell, &[(0.0, 0.0, w, 1.0)], color);
        }
        0x2590 => &[(0.5, 0.0, 1.0, 1.0)],
        0x2591..=0x2593 => {
            alpha = (n - 0x2590) as f32 * 0.25;
            &[(0.0, 0.0, 1.0, 1.0)]
        }
        0x2594 => &[(0.0, 0.0, 1.0, 0.125)],
        0x2595 => &[(0.875, 0.0, 1.0, 1.0)],
        0x2596 => &[LL],
        0x2597 => &[LR],
        0x2598 => &[UL],
        0x2599 => &[UL, LL, LR],
        0x259A => &[UL, LR],
        0x259B => &[UL, UR, LL],
        0x259C => &[UL, UR, LR],
        0x259D => &[UR],
        0x259E => &[UR, LL],
        0x259F => &[UR, LL, LR],
        _ => return false,
    };
    fill(p, cell, parts, color.gamma_multiply(alpha))
}

/// Paints a Braille pattern (U+2800–U+28FF) as a 2×4 dot grid. Many fonts (including the bundled
/// JetBrains Mono) lack these, yet CLI spinners (npm, cargo, …) use them constantly.
fn paint_braille(p: &Painter, cell: Rect, bits: u8, color: Color32) -> bool {
    // Bit → (column, row), per the Unicode dot numbering 1-2-3-7 (left) and 4-5-6-8 (right).
    const DOTS: [(f32, f32); 8] = [(0., 0.), (0., 1.), (0., 2.), (1., 0.), (1., 1.), (1., 2.), (0., 3.), (1., 3.)];
    let (o, s) = (cell.min, cell.size());
    let r = (s.x * 0.14).min(s.y * 0.07).max(0.75);
    for (i, &(col, row)) in DOTS.iter().enumerate() {
        if bits & (1 << i) != 0 {
            let center = o + vec2(s.x * (0.3 + 0.4 * col), s.y * (0.2 + 0.2 * row));
            p.circle_filled(center, r, color);
        }
    }
    true
}

fn fill(p: &Painter, cell: Rect, parts: &[(f32, f32, f32, f32)], color: Color32) -> bool {
    let (o, s) = (cell.min, cell.size());
    for &(x0, y0, x1, y1) in parts {
        // Snap to physical pixels so neighbouring cells/rects tile without seams.
        let r = Rect::from_min_max(o + vec2(x0 * s.x, y0 * s.y), o + vec2(x1 * s.x, y1 * s.y));
        p.rect_filled(p.round_rect_to_pixels(r), 0.0, color);
    }
    true
}

/// Per-pane animated cursor state.
#[derive(Default)]
pub struct CursorAnim {
    /// Leading and trailing edges of the cursor shape; the tail lags to give a smear while it moves.
    head: Option<Rect>,
    tail: Option<Rect>,
    /// Last target shape rect, to notice moves (which restart the blink).
    target: Option<Rect>,
    moved_at: f64,
}

pub struct Frame<'a> {
    pub painter: &'a Painter,
    pub origin: Pos2,
    pub theme: &'a Theme,
    pub cell: Vec2,
    pub focused: bool,
    pub faux_bold: bool,
    /// Smoothing factors for this frame's cursor movement (0..1): leading edge, trailing edge.
    pub cursor_k: f32,
    pub cursor_tail_k: f32,
    /// Forces this cursor shape instead of the one the program asked for.
    pub cursor_shape: Option<CursorShape>,
    /// Seconds since start, for the blink.
    pub time: f64,
    pub blink: bool,
}

/// Solid for this long after the cursor moves, then fades in and out.
const BLINK_HOLD: f64 = 0.6;
const BLINK_PERIOD: f64 = 1.1;
/// Stop blinking (stay solid) after this long without movement, to let the app go idle.
const BLINK_TIMEOUT: f64 = 15.0;

/// Cursor opacity at `t` seconds since it last moved, and seconds until the next frame is needed.
fn blink_alpha(t: f64) -> (f32, f32) {
    if t < BLINK_HOLD {
        return (1.0, (BLINK_HOLD - t) as f32);
    }
    if t > BLINK_TIMEOUT {
        return (1.0, f32::INFINITY);
    }
    let phase = ((t - BLINK_HOLD) / BLINK_PERIOD).fract();
    let c = (phase * std::f64::consts::TAU).cos() as f32;
    // Smoothstep around the cosine gives a plateau at both ends and a soft fade between.
    let x = ((c + 0.6) / 1.2).clamp(0.0, 1.0);
    let a = x * x * (3.0 - 2.0 * x);
    // Plateaus don't need frames.
    let next = if a == 0.0 || a == 1.0 { 1.0 / 30.0 } else { 0.0 };
    (0.08 + 0.92 * a, next)
}

/// The rect a cursor of `shape` covers inside cell `r`.
fn shape_rect(r: Rect, shape: CursorShape) -> Rect {
    match shape {
        CursorShape::Beam => Rect::from_min_size(r.min, vec2(2.0, r.height())),
        CursorShape::Underline => Rect::from_min_max(pos2(r.min.x, r.max.y - 2.0), r.max),
        _ => r,
    }
}

/// Convex hull of two rects (monotone chain), clockwise in screen space.
fn hull(a: Rect, b: Rect) -> Vec<Pos2> {
    let mut pts: Vec<Pos2> = [a, b].iter().flat_map(|r| [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()]).collect();
    pts.sort_by(|p, q| p.x.total_cmp(&q.x).then(p.y.total_cmp(&q.y)));
    let cross = |o: Pos2, a: Pos2, b: Pos2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut h: Vec<Pos2> = Vec::with_capacity(9);
    for pass in 0..2 {
        let start = h.len();
        let iter: Box<dyn Iterator<Item = &Pos2>> = if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &p in iter {
            while h.len() >= start + 2 && cross(h[h.len() - 2], h[h.len() - 1], p) <= 0.0 {
                h.pop();
            }
            h.push(p);
        }
        h.pop();
    }
    h
}

/// Paints the grid. Returns seconds until the cursor needs another frame (0 = now, infinity = never).
pub fn paint_terminal(
    f: &Frame,
    term: &Term<Listener>,
    glyphs: &mut GlyphCache,
    cursor_anim: &mut CursorAnim,
) -> f32 {
    let (p, theme, cell) = (f.painter, f.theme, f.cell);
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let rows = term.grid().screen_lines() as i32;
    let selection = content.selection;
    let mut cursor = content.cursor;
    if let Some(shape) = f.cursor_shape.filter(|_| cursor.shape != CursorShape::Hidden) {
        cursor.shape = shape;
    }
    let colors = content.colors;

    let mut reqs: Vec<GlyphReq> = Vec::with_capacity(4096);
    // (cell rect, index of its glyph in reqs, fg, bg)
    let mut cursor_cell: Option<(Rect, Option<usize>, Color32)> = None;

    for indexed in content.display_iter {
        let point = indexed.point;
        let row = point.line.0 + offset;
        if row < 0 || row >= rows {
            continue;
        }
        let c = indexed.cell;
        let flags = c.flags;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }

        let bold = flags.contains(Flags::BOLD);
        let mut fg = theme.resolve(c.fg, colors, bold);
        let mut bg = theme.resolve(c.bg, colors, false);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::DIM) {
            fg = Theme::dim(fg);
        }
        if flags.contains(Flags::HIDDEN) {
            fg = bg;
        }

        let span = if flags.contains(Flags::WIDE_CHAR) { 2.0 } else { 1.0 } * cell.x;
        let pos = f.origin + vec2(point.column.0 as f32 * cell.x, row as f32 * cell.y);
        let rect = Rect::from_min_size(pos, vec2(span, cell.y));

        if selection.is_some_and(|s| s.contains(point)) {
            p.rect_filled(rect, 0.0, theme.selection);
        } else if bg != theme.bg {
            p.rect_filled(rect, 0.0, c32(bg));
        }

        let fg32 = c32(fg);
        if flags.contains(Flags::UNDERLINE) {
            let y = rect.max.y - 1.5;
            p.line_segment([pos2(rect.min.x, y), pos2(rect.max.x, y)], Stroke::new(1.0_f32, fg32));
        }
        if flags.contains(Flags::STRIKEOUT) {
            let y = rect.center().y;
            p.line_segment([pos2(rect.min.x, y), pos2(rect.max.x, y)], Stroke::new(1.0_f32, fg32));
        }

        let visible = c.c != ' ' && c.c != '\t';
        // Block elements are drawn as exact cell-filling rects: font glyphs rarely cover the
        // full cell height (line_height > 1, ascent/descent), which leaves seams between rows.
        let block = visible && paint_block(p, rect, c.c, fg32);
        let is_cursor = point == cursor.point && cursor.shape != CursorShape::Hidden;
        if is_cursor {
            cursor_cell = Some((rect, (visible && !block).then_some(reqs.len()), c32(bg)));
        }
        if visible && !block {
            reqs.push(GlyphReq {
                pos,
                span,
                c: c.c,
                combining: c.zerowidth().map(|zw| std::iter::once(c.c).chain(zw.iter().copied()).collect()),
                color: fg32,
                bold,
            });
        }
    }

    // Resolve all glyphs with one fonts lock; only unseen glyphs get laid out.
    let galleys: Vec<Arc<Galley>> = p.ctx().fonts(|fonts| {
        reqs.iter()
            .map(|r| match &r.combining {
                Some(s) => {
                    let font = glyphs.font(r.bold);
                    if r.c != TOFU && !fonts.has_glyph(&font, r.c) {
                        glyphs.missing.push(r.c);
                    }
                    fonts.layout_no_wrap(s.clone(), font, Color32::WHITE)
                }
                None => {
                    let font = glyphs.font(r.bold);
                    let missing = &mut glyphs.missing;
                    glyphs
                        .map
                        .entry((r.c, r.bold))
                        .or_insert_with(|| {
                            if r.c != TOFU && !fonts.has_glyph(&font, r.c) {
                                missing.push(r.c);
                            }
                            fonts.layout_no_wrap(r.c.to_string(), font, Color32::WHITE)
                        })
                        .clone()
                }
            })
            .collect()
    });

    let place = |r: &GlyphReq, g: &Arc<Galley>| {
        pos2(r.pos.x + ((r.span - g.size().x) * 0.5).max(0.0), r.pos.y + ((cell.y - g.size().y) * 0.5).max(0.0).round())
    };
    for (r, g) in reqs.iter().zip(&galleys) {
        let at = place(r, g);
        p.galley_with_override_text_color(at, g.clone(), r.color);
        if r.bold && f.faux_bold {
            p.galley_with_override_text_color(at + vec2(0.6, 0.0), g.clone(), r.color);
        }
    }

    // ---- cursor
    let Some((cell_rect, glyph, cell_bg)) = cursor_cell else {
        *cursor_anim = CursorAnim::default();
        return f32::INFINITY;
    };
    let cc = c32(theme.cursor);
    let target = shape_rect(cell_rect, cursor.shape);
    if !f.focused {
        *cursor_anim = CursorAnim { head: Some(target), tail: Some(target), target: Some(target), moved_at: f.time };
        p.rect_stroke(cell_rect.shrink(0.5), 2.0, Stroke::new(1.0_f32, cc.gamma_multiply(0.7)));
        return f32::INFINITY;
    }

    if cursor_anim.target != Some(target) {
        cursor_anim.target = Some(target);
        cursor_anim.moved_at = f.time;
    }
    let settle = |cur: Option<Rect>, k: f32| {
        let r = lerp_rect(cur.unwrap_or(target), target, k);
        if (r.min - target.min).length() + (r.max - target.max).length() < 0.5 { target } else { r }
    };
    let head = settle(cursor_anim.head, f.cursor_k);
    let tail = settle(cursor_anim.tail, f.cursor_tail_k);
    cursor_anim.head = Some(head);
    cursor_anim.tail = Some(tail);
    let arrived = head == target && tail == target;

    let (alpha, next) = if f.blink && arrived { blink_alpha(f.time - cursor_anim.moved_at) } else { (1.0, f32::INFINITY) };
    let col = cc.gamma_multiply(alpha);

    if !arrived {
        // Smear: the hull of the leading and trailing edges, slightly faded behind the head.
        let trail = hull(head, tail);
        p.add(egui::Shape::convex_polygon(trail, cc.gamma_multiply(0.55), Stroke::NONE));
        p.rect_filled(head, 1.0, cc);
        return 0.0;
    }

    match cursor.shape {
        CursorShape::HollowBlock => {
            p.rect_stroke(head.shrink(0.5), 2.0, Stroke::new(1.0_f32, col));
        }
        CursorShape::Beam | CursorShape::Underline => {
            p.rect_filled(head, 1.0, col);
        }
        _ => {
            p.rect_filled(head, 2.0, col);
            if let Some(i) = glyph {
                // Re-draw the glyph under the block in the cell's background color, fading with the block.
                let inv = if cell_bg == cc { c32(theme.bg) } else { cell_bg };
                let g = crate::theme::lerp_color(reqs[i].color, inv, alpha);
                p.galley_with_override_text_color(place(&reqs[i], &galleys[i]), galleys[i].clone(), g);
            }
        }
    }
    next
}

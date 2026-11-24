use std::collections::HashMap;
use std::sync::Arc;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::CursorShape;
use egui::{pos2, vec2, Color32, FontFamily, FontId, Galley, Painter, Pos2, Rect, Stroke, Vec2};

use crate::fonts::MONO_BOLD;
use crate::pane::Listener;
use crate::theme::{c32, lerp_rect, Theme};

/// Laid-out single glyphs, reused across frames (text layout is the expensive part).
pub struct GlyphCache {
    map: HashMap<(char, bool), Arc<Galley>>,
    size: f32,
}

impl GlyphCache {
    pub fn new(size: f32) -> Self {
        Self { map: HashMap::new(), size }
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
    rect: Option<Rect>,
}

pub struct Frame<'a> {
    pub painter: &'a Painter,
    pub origin: Pos2,
    pub theme: &'a Theme,
    pub cell: Vec2,
    pub focused: bool,
    pub faux_bold: bool,
    /// Smoothing factor for this frame's cursor movement (0..1).
    pub cursor_k: f32,
    /// Forces this cursor shape instead of the one the program asked for.
    pub cursor_shape: Option<CursorShape>,
}

/// Paints the grid. Returns true while the cursor is still gliding.
pub fn paint_terminal(
    f: &Frame,
    term: &Term<Listener>,
    glyphs: &mut GlyphCache,
    cursor_anim: &mut CursorAnim,
) -> bool {
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
                Some(s) => fonts.layout_no_wrap(s.clone(), glyphs.font(r.bold), Color32::WHITE),
                None => {
                    let font = glyphs.font(r.bold);
                    glyphs
                        .map
                        .entry((r.c, r.bold))
                        .or_insert_with(|| fonts.layout_no_wrap(r.c.to_string(), font, Color32::WHITE))
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
    let Some((target, glyph, cell_bg)) = cursor_cell else {
        cursor_anim.rect = None;
        return false;
    };
    let cc = c32(theme.cursor);
    if !f.focused {
        cursor_anim.rect = Some(target);
        p.rect_stroke(target.shrink(0.5), 2.0, Stroke::new(1.0_f32, cc.gamma_multiply(0.7)));
        return false;
    }

    let cur = lerp_rect(cursor_anim.rect.unwrap_or(target), target, f.cursor_k);
    let arrived = (cur.min - target.min).length() < 0.5;
    let cur = if arrived { target } else { cur };
    cursor_anim.rect = Some(cur);

    match cursor.shape {
        CursorShape::Beam => {
            p.rect_filled(Rect::from_min_size(cur.min, vec2(2.0, cur.height())), 1.0, cc);
        }
        CursorShape::Underline => {
            p.rect_filled(Rect::from_min_max(pos2(cur.min.x, cur.max.y - 2.0), cur.max), 1.0, cc);
        }
        CursorShape::HollowBlock => {
            p.rect_stroke(cur.shrink(0.5), 2.0, Stroke::new(1.0_f32, cc));
        }
        _ => {
            p.rect_filled(cur, 2.0, cc);
            if let (true, Some(i)) = (arrived, glyph) {
                // Re-draw the glyph under the block in the cell's background color.
                let inv = if cell_bg == cc { c32(theme.bg) } else { cell_bg };
                p.galley_with_override_text_color(place(&reqs[i], &galleys[i]), galleys[i].clone(), inv);
            }
        }
    }
    !arrived
}

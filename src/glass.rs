//! "Glass" surfaces: soft shadow, a diagonal gloss, a light rim along the top edge and a shade
//! along the bottom, so panels read as frosted layers floating over a warm backdrop.
//! Also the sliding "pick" indicator and the gradient orb used by the title bar.

use egui::epaint::{RectShape, Shadow};
use egui::{pos2, vec2, Color32, ColorImage, Context, Id, Margin, Painter, Pos2, Rect, Rounding, Shape, Stroke, TextureHandle, TextureId, TextureOptions};

use crate::theme::{lerp_color, Theme};

#[derive(Clone)]
struct Textures {
    gloss: TextureHandle,
    radial: TextureHandle,
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn textures(ctx: &Context) -> Textures {
    let id = Id::new("s-term-glass-tex");
    if let Some(t) = ctx.data(|d| d.get_temp::<Textures>(id)) {
        return t;
    }
    const N: usize = 160;
    let img = |f: &dyn Fn(f32, f32) -> f32| {
        let mut px = Vec::with_capacity(N * N * 4);
        for y in 0..N {
            for x in 0..N {
                let a = (f((x as f32 + 0.5) / N as f32, (y as f32 + 0.5) / N as f32).clamp(0.0, 1.0) * 255.0).round() as u8;
                px.extend_from_slice(&[255, 255, 255, a]);
            }
        }
        ColorImage::from_rgba_unmultiplied([N, N], &px)
    };
    // Sheen: a soft glow from the top-left corner that eases out to nothing (no visible edge).
    // It's mapped at a fixed pixel size, never stretched, and the last texels are fully clear.
    let gloss = img(&|u, v| {
        // Smoothstep on both ends: no bright point at the corner and no visible rim where it ends.
        let d = (u * u + v * v).sqrt();
        let t = smooth(1.0 - d);
        t * t
    });
    let radial = img(&|u, v| {
        let d = ((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt() * 2.0;
        (1.0 - d).max(0.0).powf(1.5)
    });
    let opts = TextureOptions { wrap_mode: egui::TextureWrapMode::ClampToEdge, ..TextureOptions::LINEAR };
    let t = Textures { gloss: ctx.load_texture("s-term-gloss", gloss, opts), radial: ctx.load_texture("s-term-radial", radial, opts) };
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

fn textured(rect: Rect, rounding: impl Into<Rounding>, tint: Color32, tex: TextureId) -> Shape {
    let mut s = RectShape::filled(rect, rounding, tint);
    s.fill_texture_id = tex;
    s.uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    Shape::Rect(s)
}

/// The sheen over `r`: the gradient covers a square of fixed size from the top-left corner,
/// so wide or tall panels never get a stretched, sharp-looking band.
fn sheen_shape(ctx: &Context, r: Rect, rounding: impl Into<Rounding>, tint: Color32) -> Shape {
    let size = (r.width().max(r.height()) * 0.9).clamp(140.0, 760.0);
    let mut s = RectShape::filled(r, rounding, tint);
    s.fill_texture_id = textures(ctx).gloss.id();
    s.uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(r.width() / size, r.height() / size));
    Shape::Rect(s)
}

/// A soft radial glow (alpha falls off to nothing at the edge of `rect`).
pub fn glow(ctx: &Context, rect: Rect, color: Color32) -> Shape {
    let r = rect.width().min(rect.height()) / 2.0;
    textured(rect, r, color, textures(ctx).radial.id())
}

/// How lifted a surface is: shadow strength and size.
#[derive(Clone, Copy)]
pub enum Lift {
    Flat,
    High,
}

/// All the shapes of one glass surface, back to front.
pub fn shapes(ctx: &Context, r: Rect, rounding: f32, fill: Color32, theme: &Theme, lift: Lift) -> Vec<Shape> {
    let mut out = Vec::with_capacity(12);
    let (off, blur, a) = match lift {
        Lift::Flat => (0.0, 0.0, 0),
        Lift::High => (18.0, 56.0, 150),
    };
    if a > 0 {
        out.push(Shadow { offset: vec2(0.0, off), blur, spread: 0.0, color: Color32::from_black_alpha(a) }.as_shape(r, rounding).into());
    }
    out.push(Shape::rect_filled(r, rounding, fill));
    out.push(sheen_shape(ctx, r, rounding, theme.gloss));
    out.push(Shape::rect_stroke(r.shrink(0.5), rounding, Stroke::new(1.0_f32, theme.rim)));
    out
}

/// Just the diagonal sheen, for surfaces filled some other way (terminal panes). Paint it under text.
pub fn sheen(p: &Painter, r: Rect, rounding: impl Into<Rounding>, theme: &Theme, strength: f32) {
    p.add(sheen_shape(p.ctx(), r, rounding, theme.gloss.gamma_multiply(strength)));
}

/// Paint a glass surface.
pub fn paint(p: &Painter, r: Rect, rounding: f32, fill: Color32, theme: &Theme, lift: Lift) {
    p.extend(shapes(p.ctx(), r, rounding, fill, theme, lift));
    paint_rim(p, r, rounding, theme, 1.0);
}

/// Rim lighting: a faint light on the top edge that eases out over a short, fixed distance
/// (so it hugs the corners instead of running down tall sides), and a fainter shade at the bottom.
pub fn paint_rim(p: &Painter, r: Rect, rounding: f32, theme: &Theme, strength: f32) {
    let rr = r.shrink(0.75);
    const STEPS: usize = 14;
    let fade = |len: f32, color: Color32, from_top: bool| {
        let len = len.min(rr.height() * 0.5);
        let step = len / STEPS as f32;
        for i in 0..STEPS {
            // Smooth ease-out; adjacent bands differ by only a few percent, so no visible steps.
            let k = 1.0 - smooth((i as f32 + 0.5) / STEPS as f32);
            let (y0, y1) = if from_top {
                (rr.min.y + step * i as f32, rr.min.y + step * (i + 1) as f32)
            } else {
                (rr.max.y - step * (i + 1) as f32, rr.max.y - step * i as f32)
            };
            let y0 = if i == 0 && from_top { y0 - 2.0 } else { y0 };
            let y1 = if i == 0 && !from_top { y1 + 2.0 } else { y1 };
            let clip = Rect::from_min_max(pos2(r.min.x - 2.0, y0), pos2(r.max.x + 2.0, y1));
            p.with_clip_rect(clip.intersect(p.clip_rect())).rect_stroke(rr, rounding, Stroke::new(1.0_f32, color.gamma_multiply(k * strength)));
        }
    };
    fade(rounding * 2.2 + 10.0, theme.edge_light, true);
    fade(rounding * 1.6 + 6.0, theme.edge_shade, false);
}

/// A frame whose background is a glass surface painted behind its contents.
pub fn frame<R>(ui: &mut egui::Ui, theme: &Theme, rounding: f32, fill: Color32, lift: Lift, margin: impl Into<Margin>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let bg = ui.painter().add(Shape::Noop);
    let resp = egui::Frame::none().inner_margin(margin).show(ui, add);
    let r = resp.response.rect;
    ui.painter().set(bg, Shape::Vec(shapes(ui.ctx(), r, rounding, fill, theme, lift)));
    paint_rim(ui.painter(), r, rounding, theme, 1.0);
    resp.inner
}

/// The warm "scene" behind all the glass: the base color plus large accent glows, all kept
/// inside the window's rounded corners.
pub fn backdrop(ctx: &Context, p: &Painter, screen: Rect, rounding: f32, theme: &Theme) {
    p.rect_filled(screen, rounding, theme.bar);
    let s = screen.width().max(screen.height());
    let radial = textures(ctx).radial.id();
    // Each glow is the window-shaped rect, with the radial texture mapped onto `g`; the texture is
    // clear at its edges and clamps, so the glow stays soft and never leaks past the corners.
    let glow_in = |g: Rect, color: Color32| {
        let mut sh = RectShape::filled(screen, rounding, color);
        sh.fill_texture_id = radial;
        let uv = |pt: Pos2| pos2((pt.x - g.min.x) / g.width(), (pt.y - g.min.y) / g.height());
        sh.uv = Rect::from_min_max(uv(screen.min), uv(screen.max));
        p.add(Shape::Rect(sh));
    };
    glow_in(Rect::from_center_size(screen.left_top() + vec2(screen.width() * 0.12, screen.height() * 0.05), vec2(s * 1.3, s * 1.3)), theme.accent.gamma_multiply(0.11));
    glow_in(Rect::from_center_size(screen.right_bottom() - vec2(screen.width() * 0.08, 0.0), vec2(s * 1.1, s * 1.1)), theme.accent2.gamma_multiply(0.08));
    glow_in(Rect::from_center_size(screen.center_top() + vec2(screen.width() * 0.18, 0.0), vec2(s * 0.7, s * 0.5)), theme.warm.gamma_multiply(0.07));
}

/// A vertical hairline that fades out toward both ends instead of stopping abruptly.
pub fn fade_line_v(p: &Painter, x: f32, y0: f32, y1: f32, color: Color32) {
    let fade = ((y1 - y0) * 0.25).min(120.0);
    const STEPS: usize = 16;
    let step = fade / STEPS as f32;
    for i in 0..STEPS {
        let k = smooth((i as f32 + 0.5) / STEPS as f32);
        let c = color.gamma_multiply(k);
        p.line_segment([pos2(x, y0 + step * i as f32), pos2(x, y0 + step * (i + 1) as f32)], Stroke::new(1.0_f32, c));
        p.line_segment([pos2(x, y1 - step * (i + 1) as f32), pos2(x, y1 - step * i as f32)], Stroke::new(1.0_f32, c));
    }
    p.line_segment([pos2(x, y0 + fade), pos2(x, y1 - fade)], Stroke::new(1.0_f32, color));
}

/// Exponentially glide a rect toward `target`; snaps on first sight. Returns the shown rect.
pub fn slide(ctx: &Context, id: Id, target: Rect, animate: bool) -> Rect {
    let dt = ctx.input(|i| i.stable_dt).min(0.1);
    let cur = ctx.data(|d| d.get_temp::<Rect>(id)).unwrap_or(target);
    let k = if animate { 1.0 - (-dt * 17.0).exp() } else { 1.0 };
    let lerp = |a: Pos2, b: Pos2| a + (b - a) * k;
    let mut next = Rect::from_min_max(lerp(cur.min, target.min), lerp(cur.max, target.max));
    if (next.min - target.min).length() < 0.3 && (next.max - target.max).length() < 0.3 {
        next = target;
    } else {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(id, next));
    next
}

/// The pill marking the selected item in a dock or tab strip: the same glass surface as the
/// title bar's "Ask S-term" omnibox, so selections and the omnibox read as one family.
pub fn pick(p: &Painter, r: Rect, theme: &Theme) {
    paint(p, r, r.height() / 2.0, theme.pick, theme, Lift::Flat);
}

/// The glossy gradient orb (the dashboard's "ask" bubble), tinted with the theme's accents.
pub fn orb(p: &Painter, c: Pos2, r: f32, theme: &Theme) {
    let ctx = p.ctx();
    p.circle_filled(c, r + 3.0, theme.accent.gamma_multiply(0.13));
    p.add(Shadow { offset: vec2(0.0, 2.0), blur: 8.0, spread: 0.0, color: Color32::from_black_alpha(90) }.as_shape(Rect::from_center_size(c, vec2(r * 2.0, r * 2.0)), r));
    p.circle_filled(c, r, lerp_color(theme.accent, Color32::BLACK, 0.55));
    // Big soft accent body, lower right.
    let off = vec2(r * 0.22, r * 0.12);
    let br = r - off.length();
    p.add(glow(ctx, Rect::from_center_size(c + off, vec2(br, br) * 2.0), theme.accent));
    p.add(glow(ctx, Rect::from_center_size(c + off, vec2(br, br) * 2.0), lerp_color(theme.accent, Color32::WHITE, 0.35).gamma_multiply(0.6)));
    // Cool highlight, upper left, with a white specular core.
    let off2 = vec2(-r * 0.28, -r * 0.34);
    let hr = r - off2.length();
    p.add(glow(ctx, Rect::from_center_size(c + off2, vec2(hr, hr) * 2.0), theme.accent2));
    p.add(glow(ctx, Rect::from_center_size(c + off2, vec2(hr, hr) * 1.0), Color32::from_white_alpha(140)));
    p.circle_stroke(c, r - 0.4, Stroke::new(0.75_f32, Color32::from_white_alpha(90)));
}

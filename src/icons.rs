//! A small line-icon set drawn as vector strokes (crisp at any size, colored by theme).

use std::f32::consts::{PI, TAU};

use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum Icon {
    Files,
    Git,
    Sparkle,
    Gear,
    Search,
    Plus,
    Close,
    SplitRight,
    SplitDown,
    Terminal,
    Minimize,
    Maximize,
    Restore,
    Broadcast,
    Panel,
    Zoom,
    Folder,
    ChevronRight,
    ChevronDown,
    Palette,
    Keyboard,
    Info,
    Text,
    Image,
}

struct Pen<'a> {
    p: &'a Painter,
    o: Pos2,
    s: f32,
    stroke: Stroke,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.o + vec2(x, y) * self.s
    }
    fn line(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.p.add(Shape::line(v, self.stroke));
    }
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
        self.p.rect_stroke(Rect::from_min_max(self.at(x0, y0), self.at(x1, y1)), r * self.s, self.stroke);
    }
    fn circle(&self, x: f32, y: f32, r: f32) {
        self.p.circle_stroke(self.at(x, y), r * self.s, self.stroke);
    }
    fn dot(&self, x: f32, y: f32, r: f32) {
        self.p.circle_filled(self.at(x, y), r * self.s, self.stroke.color);
    }
    fn arc(&self, x: f32, y: f32, r: f32, a0: f32, a1: f32) {
        let n = 16;
        let v: Vec<Pos2> = (0..=n).map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            self.at(x + r * a.cos(), y + r * a.sin())
        }).collect();
        self.p.add(Shape::line(v, self.stroke));
    }
}

/// Draws `icon` centered in `rect` (designed on a 16×16 grid).
pub fn draw(p: &Painter, icon: Icon, rect: Rect, color: Color32) {
    let side = rect.width().min(rect.height());
    let s = side / 16.0;
    let o = rect.center() - vec2(8.0, 8.0) * s;
    let pen = Pen { p, o, s, stroke: Stroke::new((1.4 * s).max(1.0), color) };
    match icon {
        Icon::Files => {
            pen.line(&[(9.5, 2.0), (4.0, 2.0), (4.0, 14.0), (12.5, 14.0), (12.5, 5.0), (9.5, 2.0), (9.5, 5.0), (12.5, 5.0)]);
            pen.line(&[(6.3, 8.5), (10.2, 8.5)]);
            pen.line(&[(6.3, 11.0), (10.2, 11.0)]);
        }
        Icon::Folder => {
            pen.line(&[(2.0, 4.0), (6.0, 4.0), (7.5, 5.5), (14.0, 5.5), (14.0, 12.5), (2.0, 12.5), (2.0, 4.0)]);
        }
        Icon::Git => {
            pen.circle(5.0, 3.6, 1.6);
            pen.circle(5.0, 12.4, 1.6);
            pen.circle(11.0, 5.6, 1.6);
            pen.line(&[(5.0, 5.2), (5.0, 10.8)]);
            let mut pts = Vec::new();
            for i in 0..=10 {
                let t = i as f32 / 10.0;
                let (a, c, b) = ((11.0, 7.2), (11.0, 10.4), (5.6, 10.9));
                let x = (1.0 - t) * (1.0 - t) * a.0 + 2.0 * t * (1.0 - t) * c.0 + t * t * b.0;
                let y = (1.0 - t) * (1.0 - t) * a.1 + 2.0 * t * (1.0 - t) * c.1 + t * t * b.1;
                pts.push((x, y));
            }
            pen.line(&pts);
        }
        Icon::Sparkle => {
            let star = |cx: f32, cy: f32, r: f32| {
                let mut pts = Vec::new();
                for k in 0..=8 {
                    let a = k as f32 * PI / 4.0 - PI / 2.0;
                    let rr = if k % 2 == 0 { r } else { r * 0.3 };
                    pts.push((cx + rr * a.cos(), cy + rr * a.sin()));
                }
                pen.line(&pts);
            };
            star(7.0, 8.5, 5.5);
            star(12.5, 3.5, 2.2);
        }
        Icon::Gear => {
            pen.circle(8.0, 8.0, 2.2);
            pen.circle(8.0, 8.0, 4.8);
            for k in 0..8 {
                let a = k as f32 / 8.0 * TAU;
                pen.line(&[(8.0 + 4.8 * a.cos(), 8.0 + 4.8 * a.sin()), (8.0 + 6.6 * a.cos(), 8.0 + 6.6 * a.sin())]);
            }
        }
        Icon::Search => {
            pen.circle(7.0, 7.0, 4.3);
            pen.line(&[(10.2, 10.2), (13.8, 13.8)]);
        }
        Icon::Plus => {
            pen.line(&[(8.0, 3.0), (8.0, 13.0)]);
            pen.line(&[(3.0, 8.0), (13.0, 8.0)]);
        }
        Icon::Close => {
            pen.line(&[(4.2, 4.2), (11.8, 11.8)]);
            pen.line(&[(11.8, 4.2), (4.2, 11.8)]);
        }
        Icon::SplitRight => {
            pen.rect(2.0, 3.0, 14.0, 13.0, 1.8);
            pen.line(&[(8.0, 3.0), (8.0, 13.0)]);
        }
        Icon::SplitDown => {
            pen.rect(2.0, 3.0, 14.0, 13.0, 1.8);
            pen.line(&[(2.0, 8.0), (14.0, 8.0)]);
        }
        Icon::Terminal => {
            pen.rect(1.5, 2.5, 14.5, 13.5, 2.0);
            pen.line(&[(4.5, 6.0), (7.0, 8.0), (4.5, 10.0)]);
            pen.line(&[(8.5, 10.5), (11.5, 10.5)]);
        }
        Icon::Minimize => pen.line(&[(4.0, 8.5), (12.0, 8.5)]),
        Icon::Maximize => pen.rect(4.0, 4.0, 12.0, 12.0, 1.0),
        Icon::Restore => {
            pen.rect(3.5, 6.0, 10.0, 12.5, 1.0);
            pen.line(&[(6.0, 6.0), (6.0, 3.5), (12.5, 3.5), (12.5, 10.0), (10.0, 10.0)]);
        }
        Icon::Broadcast => {
            pen.dot(8.0, 8.0, 1.5);
            pen.arc(8.0, 8.0, 3.6, -0.8, 0.8);
            pen.arc(8.0, 8.0, 3.6, PI - 0.8, PI + 0.8);
            pen.arc(8.0, 8.0, 6.2, -0.7, 0.7);
            pen.arc(8.0, 8.0, 6.2, PI - 0.7, PI + 0.7);
        }
        Icon::Panel => {
            pen.rect(2.0, 3.0, 14.0, 13.0, 1.8);
            pen.line(&[(6.5, 3.0), (6.5, 13.0)]);
        }
        Icon::Zoom => {
            pen.line(&[(2.5, 6.0), (2.5, 2.5), (6.0, 2.5)]);
            pen.line(&[(10.0, 2.5), (13.5, 2.5), (13.5, 6.0)]);
            pen.line(&[(13.5, 10.0), (13.5, 13.5), (10.0, 13.5)]);
            pen.line(&[(6.0, 13.5), (2.5, 13.5), (2.5, 10.0)]);
        }
        Icon::ChevronRight => pen.line(&[(6.0, 4.0), (10.0, 8.0), (6.0, 12.0)]),
        Icon::ChevronDown => pen.line(&[(4.0, 6.0), (8.0, 10.0), (12.0, 6.0)]),
        Icon::Palette => {
            pen.circle(8.0, 8.0, 6.0);
            pen.dot(5.5, 6.0, 1.1);
            pen.dot(8.5, 4.8, 1.1);
            pen.dot(11.0, 7.0, 1.1);
            pen.dot(6.0, 10.0, 1.1);
        }
        Icon::Keyboard => {
            pen.rect(1.5, 4.0, 14.5, 12.0, 1.8);
            for x in [4.5, 7.0, 9.5, 12.0] {
                pen.dot(x, 6.8, 0.7);
            }
            pen.line(&[(5.0, 9.6), (11.0, 9.6)]);
        }
        Icon::Info => {
            pen.circle(8.0, 8.0, 6.0);
            pen.dot(8.0, 5.0, 0.9);
            pen.line(&[(8.0, 7.3), (8.0, 11.2)]);
        }
        Icon::Text => {
            pen.line(&[(3.5, 4.0), (12.5, 4.0)]);
            pen.line(&[(8.0, 4.0), (8.0, 12.5)]);
        }
        Icon::Image => {
            pen.rect(2.0, 3.0, 14.0, 13.0, 1.8);
            pen.circle(10.5, 6.3, 1.2);
            pen.line(&[(2.6, 11.8), (6.2, 7.8), (9.0, 10.6), (10.6, 9.2), (13.4, 11.8)]);
        }
    }
}

/// A square icon button; returns the response. `active` keeps it highlighted.
pub fn button(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32, hover_fill: Color32, active: bool, id: egui::Id) -> egui::Response {
    let sense = egui::Sense { click: true, drag: false, focusable: false };
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), sense);
    let t = ui.ctx().animate_bool_with_time(id, resp.hovered() || active, 0.12);
    if t > 0.0 {
        ui.painter().rect_filled(rect, 6.0, hover_fill.gamma_multiply(t));
    }
    draw(ui.painter(), icon, Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), color);
    resp
}

pub fn center_rect(c: Pos2, size: f32) -> Rect {
    Rect::from_center_size(c, vec2(size, size))
}

#[allow(dead_code)]
pub fn at(x: f32, y: f32) -> Pos2 {
    pos2(x, y)
}

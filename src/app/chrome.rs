//! Window chrome: custom title bar (tabs + window controls), activity rail, status bar,
//! and edge resizing for the frameless window.

use egui::{pos2, vec2, Align2, Color32, CursorIcon, FontId, Id, Margin, Mesh, Rect, ResizeDirection, Shape, Stroke, ViewportCommand};

use super::{STerm, CLICK, CLICK_DRAG};
use crate::glass::{self, Lift};
use crate::icons::{self, Icon};
use crate::sidebar::Tab as View;
use crate::theme::{c32, lerp_color, Theme};
use crate::usage_ui;

pub const TITLE_H: f32 = 42.0;
const RAIL_W: f32 = 50.0;
const STATUS_H: f32 = 26.0;
const WINDOW_ROUND: f32 = 12.0;

enum Act {
    StartDrag,
    ToggleMax,
    Minimize,
    Close,
    Settings,
    Ask,
    NewTab,
    Select(usize),
    CloseTab(usize),
    MoveTab(usize, usize),
    View(View),
}

/// The scorpion "S" glyph, tinted with an accent → accent2 gradient.
pub(super) fn paint_logo(p: &egui::Painter, tex: egui::TextureId, r: Rect, theme: &Theme) {
    let mut mesh = Mesh::with_texture(tex);
    let (a, b) = (theme.accent, theme.accent2);
    let mid = lerp_color(a, b, 0.5);
    let corners = [(r.left_top(), pos2(0.0, 0.0), a), (r.right_top(), pos2(1.0, 0.0), mid), (r.right_bottom(), pos2(1.0, 1.0), b), (r.left_bottom(), pos2(0.0, 1.0), mid)];
    for (pt, uv, c) in corners {
        mesh.vertices.push(egui::epaint::Vertex { pos: pt, uv, color: c });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(Shape::mesh(mesh));
}

/// The scorpion in one flat color.
pub(super) fn paint_glyph(p: &egui::Painter, tex: egui::TextureId, r: Rect, color: Color32) {
    let mut mesh = Mesh::with_texture(tex);
    mesh.add_rect_with_uv(r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
    p.add(Shape::mesh(mesh));
}

/// The "Ask S-term" badge: the glossy orb with the scorpion on it.
pub(super) fn ask_badge(p: &egui::Painter, tex: egui::TextureId, c: egui::Pos2, r: f32, theme: &Theme) {
    glass::orb(p, c, r, theme);
    let g = Rect::from_center_size(c, vec2(r * 1.75, r * 1.75));
    paint_glyph(p, tex, g.translate(vec2(0.0, 0.8)), Color32::from_black_alpha(90));
    paint_glyph(p, tex, g, Color32::WHITE);
}

fn window_button(ui: &mut egui::Ui, rect: Rect, icon: Icon, theme: &Theme, danger: bool, id: &str) -> bool {
    let resp = ui.interact(rect, Id::new(id), CLICK);
    let t = ui.ctx().animate_bool_with_time(Id::new((id, "h")), resp.hovered(), 0.1);
    let fill = if danger { Color32::from_rgb(0xe5, 0x5a, 0x45) } else { theme.header_focused };
    if t > 0.0 {
        let c = Rect::from_center_size(rect.center(), vec2(30.0, 30.0));
        ui.painter().rect_filled(c, 15.0, fill.gamma_multiply(t));
    }
    let col = if danger { lerp_color(theme.muted, Color32::WHITE, t) } else { lerp_color(theme.muted, theme.text, t) };
    icons::draw(ui.painter(), icon, icons::center_rect(rect.center(), 14.0), col);
    resp.clicked()
}

/// Tab label: the shell title without its `user@host:` prefix (`scorpius@fedora:~/s-term` → `~/s-term`).
pub(super) fn tab_title(title: &str) -> &str {
    match title.split_once(':') {
        Some((who, rest)) if !rest.is_empty() && who.contains('@') && !who.contains(char::is_whitespace) => rest,
        _ => title,
    }
}

#[cfg(test)]
mod tests {
    use super::tab_title;

    #[test]
    fn strips_user_host() {
        assert_eq!(tab_title("scorpius@fedora:~/s-term"), "~/s-term");
        assert_eq!(tab_title("scorpius@fedora:~"), "~");
        assert_eq!(tab_title("vim: notes.md"), "vim: notes.md");
        assert_eq!(tab_title("claude"), "claude");
        assert_eq!(tab_title("ssh me@box: uptime"), "ssh me@box: uptime");
    }
}

/// A clickable status-bar item: optional icon + text.
fn status_item(ui: &mut egui::Ui, theme: &Theme, icon: Option<Icon>, text: &str, color: Color32, id: Id) -> egui::Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let iw = if icon.is_some() { 16.0 } else { 0.0 };
    let (r, resp) = ui.allocate_exact_size(vec2(galley.size().x + iw + 12.0, STATUS_H - 6.0), CLICK);
    let t = ui.ctx().animate_bool_with_time(id, resp.hovered(), 0.1);
    let tinted = color != theme.muted;
    if tinted {
        // A tinted badge, like a delta or status chip.
        ui.painter().rect_filled(r, r.height() / 2.0, color.gamma_multiply(0.10 + 0.08 * t));
    } else if t > 0.0 {
        ui.painter().rect_filled(r, r.height() / 2.0, theme.header.gamma_multiply(t));
    }
    let mut x = r.min.x + 6.0;
    if let Some(i) = icon {
        icons::draw(ui.painter(), i, icons::center_rect(pos2(x + 6.0, r.center().y), 12.0), color);
        x += iw;
    }
    ui.painter().galley(pos2(x, r.center().y - galley.size().y / 2.0), galley, color);
    resp
}

impl STerm {
    fn apply_acts(&mut self, ctx: &egui::Context, acts: Vec<Act>, maximized: bool) {
        for a in acts {
            match a {
                Act::StartDrag => ctx.send_viewport_cmd(ViewportCommand::StartDrag),
                Act::ToggleMax => ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
                Act::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                Act::Close => ctx.send_viewport_cmd(ViewportCommand::Close),
                Act::Settings => self.settings_open = !self.settings_open,
                Act::Ask => {
                    if self.palette.open {
                        self.palette.open = false;
                    } else if self.ask.history.is_empty() && !self.ask.busy() {
                        self.palette.open_commands();
                        self.palette.ask_first = self.ask.has_key(self.config.ai_provider);
                    } else {
                        self.palette.open_ask();
                        self.ask.focus_input = true;
                    }
                }
                Act::NewTab => self.new_tab(),
                Act::Select(i) => self.active = i.min(self.tabs.len().saturating_sub(1)),
                Act::CloseTab(i) => self.close_tab(i),
                Act::MoveTab(from, to) => {
                    if from < self.tabs.len() && to < self.tabs.len() && from != to {
                        let active_root = self.tabs[self.active].focus;
                        let t = self.tabs.remove(from);
                        self.tabs.insert(to, t);
                        self.active = self.tabs.iter().position(|t| t.focus == active_root).unwrap_or(0);
                        self.last_active = self.active;
                    }
                }
                Act::View(v) => self.sidebar.toggle_view(v),
            }
        }
    }

    // ------------------------------------------------------------------------------ title bar

    pub(super) fn title_bar(&mut self, ctx: &egui::Context) {
        let frameless = !self.config.system_titlebar;
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let resizing = self.frame_resizing;
        let mut acts = Vec::new();
        let mut plus_rect = Rect::NOTHING;
        // Tabs running a Launchpad launcher show its name.
        let launched: Vec<Option<String>> = self.tabs.iter().map(|t| self.launched(t.focus).map(String::from)).collect();
        let theme = &self.theme;

        egui::TopBottomPanel::top("s-term-title")
            .exact_height(TITLE_H)
            .show_separator_line(false)
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                // Empty title-bar space moves / maximizes the window. Added first so widgets win.
                let bg = ui.interact(bar, Id::new("s-term-titlebar"), CLICK_DRAG);
                if frameless && !resizing {
                    if bg.drag_started() {
                        acts.push(Act::StartDrag);
                    }
                    if bg.double_clicked() {
                        acts.push(Act::ToggleMax);
                    }
                }
                let p = ui.painter().clone();
                paint_logo(&p, self.logo.id(), Rect::from_center_size(pos2(bar.min.x + RAIL_W / 2.0, bar.center().y), vec2(30.0, 30.0)), theme);

                // ---- right cluster (laid out right-to-left)
                let mut x = bar.max.x;
                if frameless {
                    let w = 46.0;
                    let mk = |x: f32| Rect::from_min_max(pos2(x - w, bar.min.y), pos2(x, bar.max.y - 1.0));
                    if window_button(ui, mk(x), Icon::Close, theme, true, "s-term-wc-close") {
                        acts.push(Act::Close);
                    }
                    if self.config.window_buttons {
                        x -= w;
                        let max_icon = if maximized { Icon::Restore } else { Icon::Maximize };
                        if window_button(ui, mk(x), max_icon, theme, false, "s-term-wc-max") {
                            acts.push(Act::ToggleMax);
                        }
                        x -= w;
                        if window_button(ui, mk(x), Icon::Minimize, theme, false, "s-term-wc-min") {
                            acts.push(Act::Minimize);
                        }
                    }
                    x -= w + 6.0;
                } else {
                    x -= 8.0;
                }

                let right = Rect::from_min_max(pos2(bar.min.x, bar.min.y), pos2(x, bar.max.y));
                let mut right_used = 0.0;
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(right), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let start = ui.cursor().max.x;
                        // Command palette "omnibox".
                        let wide = bar.width() > 980.0;
                        let (pr, presp) = ui.allocate_exact_size(vec2(if wide { 270.0 } else { 32.0 }, 32.0), CLICK);
                        let ph = ui.ctx().animate_bool_with_time(Id::new("s-term-tb-pal"), presp.hovered() || self.palette.open, 0.12);
                        glass::paint(&p, pr, 16.0, lerp_color(theme.glass, theme.header_focused, ph), theme, Lift::Flat);
                        let can_ask = self.ask.has_key(self.config.ai_provider) || !self.ask.history.is_empty() || self.ask.busy();
                        if wide {
                            ask_badge(&p, self.logo.id(), pos2(pr.min.x + 16.0, pr.center().y), 10.0, theme);
                            // The prompt types itself in when the window opens.
                            let full = if self.ask.needs_attention() {
                                "S-term is waiting for your OK"
                            } else if self.ask.busy() {
                                "S-term is working…"
                            } else if can_ask {
                                "Ask S-term…"
                            } else {
                                "Search…"
                            };
                            let age = ui.input(|i| i.time) as f32;
                            let n = ((age - 0.25).max(0.0) / 0.026) as usize;
                            let shown: String = full.chars().take(n).collect();
                            if n <= full.chars().count() {
                                ui.ctx().request_repaint();
                            }
                            p.text(pos2(pr.min.x + 33.0, pr.center().y), Align2::LEFT_CENTER, shown, FontId::proportional(12.0), lerp_color(theme.muted, theme.text, 0.35 + 0.65 * ph));
                            icons::draw(&p, Icon::Search, icons::center_rect(pos2(pr.max.x - 17.0, pr.center().y), 13.0), theme.muted);
                        } else {
                            ask_badge(&p, self.logo.id(), pr.center(), 10.0, theme);
                        }
                        if presp.on_hover_text(if can_ask { "Ask S-term, or search commands (Ctrl+Shift+P)" } else { "Search commands and settings (Ctrl+Shift+P)" }).clicked() {
                            acts.push(Act::Ask);
                        }
                        right_used = start - ui.cursor().max.x;
                    });
                });

                // ---- tabs
                let tabs_x0 = bar.min.x + RAIL_W + 6.0;
                let tabs_x1 = x - right_used - 50.0;
                let font = FontId::proportional(12.5);
                let titles: Vec<String> = self
                    .tabs
                    .iter()
                    .zip(&launched)
                    .map(|(t, name)| match name {
                        Some(name) => name.clone(),
                        None => self.panes.get(&t.focus).map(|p| tab_title(&p.title).to_string()).unwrap_or_default(),
                    })
                    .collect();
                let ideal: Vec<f32> = titles.iter().map(|t| (p.layout_no_wrap(t.clone(), font.clone(), Color32::WHITE).size().x + 70.0).clamp(130.0, 240.0)).collect();
                let gap = 4.0;
                let total: f32 = ideal.iter().sum::<f32>() + gap * ideal.len() as f32;
                let scale = ((tabs_x1 - tabs_x0) / total).min(1.0);
                let mut tx = tabs_x0;
                let mut centers = Vec::new();
                let mut dragged: Option<(usize, f32)> = None;
                // Lay out first so the selection pill can glide to the active tab before tabs are painted.
                let mut tab_rects = Vec::new();
                for i in 0..self.tabs.len() {
                    let w = (ideal[i] * scale).max(64.0);
                    tab_rects.push(Rect::from_min_size(pos2(tx, bar.center().y - 16.0), vec2(w, 32.0)));
                    tx += w + gap;
                }
                if let Some(&target) = tab_rects.get(self.active) {
                    let shown = glass::slide(ui.ctx(), Id::new("s-term-tab-pick"), target, self.config.animations);
                    glass::pick(&p, shown, theme);
                }
                for (i, tab) in self.tabs.iter().enumerate() {
                    let r = tab_rects[i];
                    centers.push(r.center().x);
                    let resp = ui.interact(r, Id::new(("s-term-tab", i)), CLICK_DRAG);
                    let xr = Rect::from_center_size(pos2(r.max.x - 15.0, r.center().y), vec2(20.0, 20.0));
                    let xresp = ui.interact(xr, Id::new(("s-term-tab-x", i)), CLICK);
                    let active = i == self.active;
                    let hover = ui.ctx().animate_bool_with_time(Id::new(("s-term-tab-h", i)), resp.hovered() || xresp.hovered(), 0.12);
                    let on = ui.ctx().animate_bool_with_time(Id::new(("s-term-tab-on", i)), active, 0.18);

                    if !active && hover > 0.01 {
                        p.rect_filled(r, r.height() / 2.0, theme.header.gamma_multiply(hover));
                    }
                    let panes = {
                        let mut v = Vec::new();
                        tab.root.leaves(&mut v);
                        v.len()
                    };
                    let icon = if tab.broadcast {
                        Icon::Broadcast
                    } else if launched[i].is_some() {
                        Icon::Play
                    } else if panes > 1 {
                        Icon::SplitRight
                    } else {
                        Icon::Terminal
                    };
                    icons::draw(&p, icon, icons::center_rect(pos2(r.min.x + 18.0, r.center().y), 14.0), lerp_color(theme.muted, theme.pick_ink, on));
                    let lit = hover.max(on);
                    let clip = p.with_clip_rect(Rect::from_min_max(r.min, pos2(r.max.x - if lit > 0.05 { 28.0 } else { 10.0 }, r.max.y)));
                    let ink = lerp_color(lerp_color(theme.muted, theme.text, hover), theme.pick_ink, on);
                    // Long titles (usually paths) lose their *start*, keeping the tail visible;
                    // the full title is in the tooltip. Fixed close-button reserve so text doesn't jump on hover.
                    let shown = elide_left(&p, &titles[i], &font, r.max.x - 28.0 - (r.min.x + 33.0));
                    clip.text(pos2(r.min.x + 33.0, r.center().y), Align2::LEFT_CENTER, shown, font.clone(), ink);
                    if lit > 0.05 {
                        if xresp.hovered() {
                            p.rect_filled(xr, 10.0, if active { theme.pick_ink.gamma_multiply(0.12) } else { theme.header_focused });
                        }
                        icons::draw(&p, Icon::Close, icons::center_rect(xr.center(), 11.0), lerp_color(theme.muted, theme.pick_ink, on).gamma_multiply(lit));
                    }
                    if xresp.clicked() || resp.middle_clicked() {
                        acts.push(Act::CloseTab(i));
                    } else if resp.clicked() || resp.drag_started() {
                        acts.push(Act::Select(i));
                    }
                    if resp.dragged() {
                        if let Some(pp) = resp.interact_pointer_pos() {
                            dragged = Some((i, pp.x));
                            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                        }
                    }
                    let tip = format!("{}  ·  {panes} pane{}", titles[i], if panes == 1 { "" } else { "s" });
                    resp.on_hover_text(tip);
                }
                if let Some((from, px)) = dragged {
                    let to = centers.iter().enumerate().filter(|(j, c)| *j != from && **c < px).count();
                    if to != from {
                        acts.push(Act::MoveTab(from, to));
                    }
                }
                let plus = Rect::from_min_size(pos2(tx - 1.0, bar.center().y - 14.0), vec2(28.0, 28.0));
                plus_rect = plus;
                let presp = ui.interact(plus, Id::new("s-term-tab-plus"), CLICK);
                // Something dragged over "+" (from the Explorer or the desktop): drop opens a tab there.
                let dragging = self.sidebar.dragging().is_some() || ui.input(|i| !i.raw.hovered_files.is_empty());
                let over = dragging && ui.input(|i| i.pointer.latest_pos()).is_some_and(|q| plus.expand(4.0).contains(q));
                let dh = ui.ctx().animate_bool_with_time(Id::new("s-term-tab-plus-drop"), over, 0.12);
                let ph = ui.ctx().animate_bool_with_time(Id::new("s-term-tab-plus-h"), presp.hovered(), 0.12).max(dh);
                p.rect_filled(plus, 14.0, lerp_color(theme.header_focused.gamma_multiply(ph), theme.accent.gamma_multiply(0.25), dh));
                if dh > 0.01 {
                    p.rect_stroke(plus.shrink(0.5), 14.0, egui::Stroke::new(1.5_f32, theme.accent.gamma_multiply(dh)));
                }
                icons::draw(&p, Icon::Plus, icons::center_rect(plus.center(), 14.0), lerp_color(lerp_color(theme.muted, theme.text, ph), theme.accent, dh));
                let tip = if dragging { "Drop to open a new tab here" } else { "New tab (Ctrl+Shift+T)" };
                if presp.on_hover_text(tip).clicked() {
                    acts.push(Act::NewTab);
                }
            });
        self.plus_rect = plus_rect;
        self.apply_acts(ctx, acts, maximized);
    }

    // ------------------------------------------------------------------------------ rail

    pub(super) fn rail(&mut self, ctx: &egui::Context) {
        let theme = &self.theme;
        let mut acts = Vec::new();
        let changes = self.sidebar.snapshot().and_then(|s| s.git.as_ref().map(|g| g.changes.len())).unwrap_or(0);
        let usage = self.sidebar.usage.snapshot();
        // With no AI tools tracked, the AI usage view has nothing to show.
        let ai_tools = self.config.any_ai_tool();
        let launching = self.any_launch_busy();
        if !ai_tools && self.sidebar.view() == View::Ai {
            let open = self.sidebar.open;
            self.sidebar.show_view(View::Files);
            self.sidebar.open = open;
        }
        egui::SidePanel::left("s-term-rail")
            .exact_width(RAIL_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let r = ui.max_rect();
                let p = ui.painter().clone();
                let items: Vec<_> = [
                    (View::Files, Icon::Files, "Explorer"),
                    (View::Changes, Icon::Git, "Source control"),
                    (View::Launch, Icon::Play, "Launchpad"),
                    (View::History, Icon::History, "History (Ctrl+Shift+H)"),
                    (View::Ai, Icon::Sparkle, "AI usage"),
                ]
                .into_iter()
                .filter(|(v, ..)| *v != View::Ai || ai_tools)
                .collect();
                let slot = |y: f32| Rect::from_center_size(pos2(r.center().x, y + 18.0), vec2(36.0, 36.0));
                let y0 = r.min.y + 8.0;
                // A light pill glides to the open view.
                let open = items.iter().position(|(v, ..)| self.sidebar.open && self.sidebar.view() == *v);
                let pick_on = ui.ctx().animate_bool_with_time(Id::new("s-term-rail-pick"), open.is_some(), 0.15);
                if let Some(i) = open {
                    let shown = glass::slide(ui.ctx(), Id::new("s-term-rail-pick-r"), slot(y0 + 44.0 * i as f32), self.config.animations);
                    glass::pick(&p, Rect::from_center_size(shown.center(), shown.size() * (0.6 + 0.4 * pick_on)), theme);
                }
                let mut y = y0;
                let item = |icon: Icon, active: bool, label: &str, idkey: &str, y: f32| -> egui::Response {
                    let br = slot(y);
                    let resp = ui.interact(br, Id::new(("s-term-rail", idkey)), CLICK);
                    let on = ui.ctx().animate_bool_with_time(Id::new(("s-term-rail-on", idkey)), active, 0.18);
                    let hv = ui.ctx().animate_bool_with_time(Id::new(("s-term-rail-h", idkey)), resp.hovered(), 0.1);
                    if hv > 0.0 && on < 1.0 {
                        p.rect_filled(br, 18.0, theme.header_focused.gamma_multiply(hv * (1.0 - on)));
                    }
                    let col = lerp_color(lerp_color(theme.muted, theme.text, hv), theme.pick_ink, on);
                    icons::draw(&p, icon, icons::center_rect(br.center(), 18.0), col);
                    resp.on_hover_text(label)
                };
                for (view, icon, label) in items {
                    let active = self.sidebar.open && self.sidebar.view() == view;
                    let resp = item(icon, active, label, label, y);
                    let br = resp.rect;
                    if view == View::Changes && changes > 0 {
                        let txt = if changes > 99 { "99+".to_string() } else { changes.to_string() };
                        let g = p.layout_no_wrap(txt, FontId::proportional(9.0), theme.bar);
                        let badge = Rect::from_min_size(pos2(br.max.x - g.size().x - 2.0, br.min.y - 1.0), vec2(g.size().x + 8.0, 14.0));
                        p.rect_filled(badge.expand(1.5), 8.5, theme.bar);
                        p.rect_filled(badge, 7.0, theme.accent);
                        p.galley(pos2(badge.min.x + 4.0, badge.center().y - g.size().y / 2.0), g, theme.bar);
                    }
                    if view == View::Launch && launching {
                        p.circle_filled(pos2(br.max.x - 6.0, br.min.y + 6.0), 3.5, c32(theme.palette[2]));
                    }
                    if view == View::Ai {
                        if let Some(l) = usage.as_ref().filter(|u| u.claude.running).and_then(|u| u.claude.live.as_ref()).filter(|l| l.here) {
                            if let Some(pct) = l.context_pct.or_else(|| l.context_size.map(|s| l.context_tokens as f32 / s as f32 * 100.0)) {
                                let c = if pct >= 75.0 { c32(theme.palette[1]) } else if pct >= 50.0 { c32(theme.palette[3]) } else { c32(theme.palette[2]) };
                                p.circle_filled(pos2(br.max.x - 6.0, br.min.y + 6.0), 3.5, c);
                            }
                        }
                    }
                    if resp.clicked() {
                        acts.push(Act::View(view));
                    }
                    y += 44.0;
                }
                let gy = r.max.y - 48.0;
                if self.settings_open {
                    glass::pick(&p, slot(gy), theme);
                }
                if item(Icon::Gear, self.settings_open, "Settings (Ctrl+,)", "settings", gy).clicked() {
                    acts.push(Act::Settings);
                }
            });
        self.apply_acts(ctx, acts, false);
    }

    // ------------------------------------------------------------------------------ status bar

    pub(super) fn status_bar(&mut self, ctx: &egui::Context) {
        let theme = &self.theme;
        let snap = self.sidebar.snapshot();
        let usage = self.sidebar.usage.snapshot();
        let cfg = self.config.clone();
        let mut acts = Vec::new();
        let (panes, broadcast, grid) = match self.tab() {
            Some(t) => {
                let mut v = Vec::new();
                t.root.leaves(&mut v);
                (v.len(), t.broadcast, self.panes.get(&t.focus).map(|p| (p.size.cols, p.size.rows)))
            }
            None => (0, false, None),
        };
        egui::TopBottomPanel::bottom("s-term-status")
            .exact_height(STATUS_H)
            .show_separator_line(false)
            .frame(egui::Frame::none().inner_margin(Margin::symmetric(8.0, 0.0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if let Some(s) = &snap {
                        if let Some(g) = &s.git {
                            // Long branch names are shortened here; the full name is in the tooltip.
                            let short: String = if g.branch.chars().count() > 32 { g.branch.chars().take(31).chain(['…']).collect() } else { g.branch.clone() };
                            if status_item(ui, theme, Some(Icon::Git), &short, theme.accent2, Id::new("s-term-st-branch")).on_hover_text(format!("{}  ·  Source control", g.branch)).clicked() {
                                acts.push(Act::View(View::Changes));
                            }
                            let n = g.changes.len();
                            let (t, c) = if n == 0 { ("✓ clean".to_string(), c32(theme.palette[2])) } else { (format!("● {n} changed"), c32(theme.palette[3])) };
                            if status_item(ui, theme, None, &t, c, Id::new("s-term-st-changes")).clicked() {
                                acts.push(Act::View(View::Changes));
                            }
                        }
                        let home = std::env::var("HOME").unwrap_or_default();
                        let mut d = s.dir.to_string_lossy().into_owned();
                        if !home.is_empty() && d.starts_with(&home) {
                            d = format!("~{}", &d[home.len()..]);
                        }
                        // Deep paths keep only their last two folders so they don't run into the right side.
                        let parts: Vec<&str> = d.split('/').filter(|p| !p.is_empty()).collect();
                        let short = if d.chars().count() > 32 && parts.len() > 2 { format!("…/{}", parts[parts.len() - 2..].join("/")) } else { d.clone() };
                        if status_item(ui, theme, Some(Icon::Folder), &short, theme.muted, Id::new("s-term-st-dir")).on_hover_text(format!("{d}  ·  Explorer")).clicked() {
                            acts.push(Act::View(View::Files));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let font = format!("{}px", self.config.font_size);
                        let theme_name = crate::theme::PRESETS.iter().find(|(id, _)| *id == theme.id).map_or("", |(_, n)| *n);
                        if status_item(ui, theme, Some(Icon::Palette), &format!("{theme_name} · {font}"), theme.muted, Id::new("s-term-st-theme")).on_hover_text("Appearance settings").clicked() {
                            acts.push(Act::Settings);
                        }
                        if let Some((c, rws)) = grid {
                            status_item(ui, theme, None, &format!("{c}×{rws}"), theme.muted, Id::new("s-term-st-grid"));
                        }
                        status_item(ui, theme, Some(Icon::SplitRight), &format!("{panes}"), theme.muted, Id::new("s-term-st-panes")).on_hover_text("Panes in this tab");
                        if broadcast {
                            status_item(ui, theme, Some(Icon::Broadcast), "broadcasting", theme.accent2, Id::new("s-term-st-bc"));
                        }
                        if let Some(u) = &usage {
                            if usage_ui::status_meter(ui, theme, u, &cfg, STATUS_H - 6.0).is_some_and(|r| r.clicked()) {
                                acts.push(Act::View(View::Ai));
                            }
                        }
                    });
                });
            });
        self.apply_acts(ctx, acts, false);
    }

    // ------------------------------------------------------------------------------ frame

    /// Resize the frameless window from its edges.
    pub(super) fn frame_resize(&mut self, ctx: &egui::Context) {
        self.frame_resizing = false;
        if self.config.system_titlebar || ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
            return;
        }
        let screen = ctx.screen_rect();
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let m = 6.0;
        let (l, r, t, b) = (pos.x < screen.min.x + m, pos.x > screen.max.x - m, pos.y < screen.min.y + m, pos.y > screen.max.y - m);
        let dir = match (l, r, t, b) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return,
        };
        ctx.set_cursor_icon(match dir {
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
            ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
            _ => CursorIcon::ResizeVertical,
        });
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
            self.frame_resizing = true;
        }
    }

    /// Corner radius of the frameless window: rounded when floating, square when maximized or fullscreen.
    pub(super) fn window_rounding(&self, ctx: &egui::Context) -> f32 {
        let filled = ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false));
        if self.config.system_titlebar || filled {
            0.0
        } else {
            WINDOW_ROUND
        }
    }

    /// The window's own edge: a hairline rim with soft light along the top, like the other glass.
    pub(super) fn window_border(&self, ctx: &egui::Context) {
        let rounding = self.window_rounding(ctx);
        if rounding == 0.0 {
            return;
        }
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("s-term-border")));
        let r = ctx.screen_rect();
        p.rect_stroke(r.shrink(0.5), rounding, Stroke::new(1.0_f32, lerp_color(self.theme.border, self.theme.rim, 0.5)));
        glass::paint_rim(&p, r, rounding, &self.theme, 1.2);
    }
}

/// Trim `text` from the left (prefixing "…") until it fits in `max_w` pixels.
fn elide_left(p: &egui::Painter, text: &str, font: &FontId, max_w: f32) -> String {
    let width = |t: &str| p.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x;
    if width(text) <= max_w {
        return text.to_string();
    }
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    // Binary search for the smallest cut that fits.
    let (mut lo, mut hi) = (1, starts.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if width(&format!("…{}", &text[starts[mid]..])) <= max_w {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    match starts.get(lo) {
        Some(&b) => format!("…{}", &text[b..]),
        None => "…".to_string(),
    }
}

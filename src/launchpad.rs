//! Launchpad: saved series of commands ("launchers") that start with one click.
//!
//! A launcher is a folder and a list of commands. Its play button opens a new tab (or a split, or
//! uses the focused pane) in that folder and types the commands joined with `&&`, so each one runs
//! only if the one before it succeeded. This file draws the panel and edits `Config::launchers`;
//! the app (`app/launch.rs`) owns the panes, types the commands and reports each one's `Status`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Rect, RichText, Sense, Stroke};

use crate::app::CLICK;
use crate::config::{Config, Launcher, RunIn};
use crate::icons::{self, Icon};
use crate::sidebar::UiAction;
use crate::theme::{c32, lerp_color, Theme};

const ROW_H: f32 = 48.0;

/// What the panel asks the app to do with a launcher.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Run,
    /// Ctrl+C in its pane.
    Stop,
    /// Ctrl+C, then run again once the shell is back at its prompt.
    Restart,
    /// Focus its pane.
    Show,
    /// Close its pane.
    Close,
}

#[derive(Clone, PartialEq, Debug)]
pub enum State {
    /// The commands are about to be typed (the new shell's prompt isn't up yet).
    Starting,
    /// This program is in the foreground.
    Running(String),
    /// The shell is back at its prompt.
    Finished,
}

/// What a launcher's pane is doing, as the app last saw it.
#[derive(Clone, PartialEq, Debug)]
pub struct Status {
    pub state: State,
    /// 1-based number of the tab its pane is in.
    pub tab: usize,
}

impl Status {
    pub fn busy(&self) -> bool {
        self.state != State::Finished
    }
}

/// Launcher id → status, for launchers that have a pane.
pub type Statuses = HashMap<u64, Status>;

// ------------------------------------------------------------------------------------- commands

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// The launcher's folder with `~` expanded, or None when it has none.
pub fn expand(dir: &str) -> Option<PathBuf> {
    let d = dir.trim();
    if d.is_empty() {
        return None;
    }
    match (d.strip_prefix('~'), home()) {
        (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with('/') => Some(h.join(rest.trim_start_matches('/'))),
        _ => Some(PathBuf::from(d)),
    }
}

/// `/home/me/x` → `~/x`.
pub fn tilde(p: &Path) -> String {
    match home().and_then(|h| p.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// The commands that run, in order. Blank lines and `#` comments are left out.
pub fn steps(l: &Launcher) -> Vec<&str> {
    l.commands.iter().map(|c| c.trim()).filter(|c| !c.is_empty() && !c.starts_with('#')).collect()
}

/// The line typed at the prompt: `cd <folder> && step && step`. Without `cd` when the shell
/// already starts in the folder.
pub fn command_line(l: &Launcher, cd: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = expand(&l.dir).filter(|_| cd) {
        parts.push(format!("cd {}", crate::sidebar::quote(&d.to_string_lossy())));
    }
    parts.extend(steps(l).into_iter().map(String::from));
    parts.join(" && ")
}

/// An id no launcher has yet.
fn new_id(cfg: &Config) -> u64 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |d| d.as_millis() as u64);
    now.max(cfg.launchers.iter().map(|l| l.id).max().unwrap_or(0) + 1)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------------------------- UI state

/// A launcher being written or edited.
struct Draft {
    /// The launcher it edits; None for a new one.
    id: Option<u64>,
    name: String,
    dir: String,
    /// One command per line.
    commands: String,
    run_in: RunIn,
    /// Put the cursor in a field on the first frame.
    focus: bool,
}

impl Draft {
    fn launcher(&self) -> Launcher {
        let name = match self.name.trim() {
            "" => expand(&self.dir).and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "Launcher".into()),
            n => n.to_string(),
        };
        let commands = self.commands.lines().map(str::trim_end).filter(|c| !c.trim().is_empty()).map(String::from).collect();
        Launcher { id: self.id.unwrap_or(0), name, dir: self.dir.trim().to_string(), commands, run_in: self.run_in }
    }
}

#[derive(Default)]
pub struct LaunchpadUi {
    draft: Option<Draft>,
    /// Folder picked in the system dialog, delivered by its thread.
    picked: Arc<Mutex<Option<PathBuf>>>,
    /// A field in the editor has the keyboard, so keys must not also reach the shell.
    pub typing: bool,
}

impl LaunchpadUi {
    /// Opens the editor on a new launcher, filled in from `dir` (the focused pane's folder, or a
    /// folder right-clicked in the Explorer).
    pub fn new_draft(&mut self, dir: Option<&Path>) {
        // The home folder or `/` is rarely what a launcher is for.
        let dir = dir.filter(|d| d.parent().is_some() && home().as_deref() != Some(*d));
        let name = dir.and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.draft = Some(Draft { id: None, name, dir: dir.map(tilde).unwrap_or_default(), commands: String::new(), run_in: RunIn::Tab, focus: true });
    }

    pub fn editing_new(&self) -> bool {
        self.draft.as_ref().is_some_and(|d| d.id.is_none())
    }

    pub fn close_editor(&mut self) {
        self.draft = None;
    }

    fn edit(&mut self, l: &Launcher) {
        self.draft = Some(Draft { id: Some(l.id), name: l.name.clone(), dir: l.dir.clone(), commands: l.commands.join("\n"), run_in: l.run_in, focus: true });
    }
}

/// A change to the list asked for from a row (applied after the list is drawn).
enum Change {
    Edit(u64),
    Duplicate(u64),
    Move(u64, isize),
    Delete(u64),
}

enum EditOut {
    Save(Launcher),
    Delete(u64),
    Cancel,
}

// ------------------------------------------------------------------------------------------ panel

/// The Launchpad view. `cwd` is the focused pane's folder, where new launchers start.
pub fn panel(ui: &mut egui::Ui, theme: &Theme, st: &mut LaunchpadUi, cfg: &mut Config, runs: &Statuses, cwd: Option<&Path>, actions: &mut Vec<UiAction>) {
    st.typing = false;
    if let Some(p) = lock(&st.picked).take() {
        if let Some(d) = &mut st.draft {
            if d.name.trim().is_empty() {
                d.name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            }
            d.dir = tilde(&p);
        }
    }
    let mut edit_out = None;
    let mut change = None;
    if st.editing_new() {
        edit_out = editor(ui, theme, st);
    }
    if cfg.launchers.is_empty() && st.draft.is_none() {
        empty_state(ui, theme, st, cwd);
    }
    let n = cfg.launchers.len();
    for (i, l) in cfg.launchers.iter().enumerate() {
        if st.draft.as_ref().is_some_and(|d| d.id == Some(l.id)) {
            edit_out = editor(ui, theme, st);
            continue;
        }
        if let Some(c) = row(ui, theme, l, runs.get(&l.id), i, n, actions) {
            change = Some(c);
        }
        ui.add_space(6.0);
    }
    if n > 0 {
        ui.add_space(4.0);
        ui.label(RichText::new("Click a running launcher to go to its tab. Right-click for more.").size(10.5).color(theme.muted));
    }

    let pos = |cfg: &Config, id: u64| cfg.launchers.iter().position(|l| l.id == id);
    match change {
        Some(Change::Edit(id)) => {
            if let Some(i) = pos(cfg, id) {
                let l = cfg.launchers[i].clone();
                st.edit(&l);
            }
        }
        Some(Change::Duplicate(id)) => {
            if let Some(i) = pos(cfg, id) {
                let mut l = cfg.launchers[i].clone();
                l.id = new_id(cfg);
                l.name = format!("{} copy", l.name);
                cfg.launchers.insert(i + 1, l);
            }
        }
        Some(Change::Move(id, by)) => {
            if let Some(i) = pos(cfg, id) {
                let j = i as isize + by;
                if (0..n as isize).contains(&j) {
                    cfg.launchers.swap(i, j as usize);
                }
            }
        }
        Some(Change::Delete(id)) => {
            cfg.launchers.retain(|l| l.id != id);
            if st.draft.as_ref().is_some_and(|d| d.id == Some(id)) {
                st.draft = None;
            }
        }
        None => {}
    }
    match edit_out {
        Some(EditOut::Save(mut l)) => {
            match pos(cfg, l.id).filter(|_| l.id != 0) {
                Some(i) => cfg.launchers[i] = l,
                None => {
                    // New launchers go on top, where their editor was.
                    l.id = new_id(cfg);
                    cfg.launchers.insert(0, l);
                }
            }
            st.draft = None;
        }
        Some(EditOut::Delete(id)) => {
            cfg.launchers.retain(|l| l.id != id);
            st.draft = None;
        }
        Some(EditOut::Cancel) => st.draft = None,
        None => {}
    }
}

/// What the panel shows before there are any launchers.
fn empty_state(ui: &mut egui::Ui, theme: &Theme, st: &mut LaunchpadUi, cwd: Option<&Path>) {
    crate::usage_ui::card(ui, theme, |ui| {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 17.0, theme.accent.gamma_multiply(0.16));
            icons::draw(ui.painter(), Icon::PlaySolid, icons::center_rect(r.center() + vec2(0.8, 0.0), 14.0), theme.accent);
            ui.vertical(|ui| {
                ui.label(RichText::new("Start a project in one click").size(13.5).strong().color(theme.text));
                ui.label(RichText::new("No more typing the same commands").size(11.5).color(theme.muted));
            });
        });
        ui.add_space(8.0);
        ui.label(
            RichText::new("A launcher remembers a folder and the commands you run there. Its play button opens a tab in the folder and runs them in order.")
                .size(11.5)
                .color(theme.muted),
        );
        ui.add_space(10.0);
        let here = cwd.filter(|d| d.parent().is_some() && home().as_deref() != Some(*d)).and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned());
        let label = match &here {
            Some(n) => format!("New launcher for {n}"),
            None => "New launcher".into(),
        };
        let btn = egui::Button::new(RichText::new(label).color(theme.bar)).fill(theme.accent).sense(CLICK);
        if ui.add(btn).clicked() {
            st.new_draft(cwd);
        }
    });
}

/// One launcher: the play/stop button, its name and what it's doing, and an edit button.
fn row(ui: &mut egui::Ui, theme: &Theme, l: &Launcher, status: Option<&Status>, i: usize, n: usize, actions: &mut Vec<UiAction>) -> Option<Change> {
    let mut change = None;
    let id = Id::new(("s-term-launch", l.id));
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), CLICK);
    // The buttons sit on top of the row, so "hovered" would go off over them.
    let hover = ui.ctx().animate_bool_with_time(id.with("h"), ui.rect_contains_pointer(r), 0.1);
    let busy = status.is_some_and(Status::busy);
    let (green, red) = (c32(theme.palette[2]), c32(theme.palette[1]));
    let p = ui.painter();
    p.rect_filled(r, 10.0, lerp_color(theme.header.gamma_multiply(0.45), theme.header, hover));
    if busy {
        p.rect_stroke(r.shrink(0.5), 10.0, Stroke::new(1.0_f32, green.gamma_multiply(0.35)));
    }

    let bc = pos2(r.min.x + 24.0, r.center().y);
    let go = ui.interact(Rect::from_center_size(bc, vec2(32.0, 32.0)), id.with("go"), CLICK);
    let gh = ui.ctx().animate_bool_with_time(id.with("go-h"), go.hovered(), 0.1);
    if busy {
        p.circle_filled(bc, 15.0, lerp_color(green.gamma_multiply(0.16), red, gh));
        p.circle_stroke(bc, 15.0, Stroke::new(1.0_f32, green.gamma_multiply(0.7 * (1.0 - gh))));
        icons::draw(p, Icon::Stop, icons::center_rect(bc, 12.0), lerp_color(green, Color32::WHITE, gh));
    } else {
        p.circle_filled(bc, 15.0, lerp_color(theme.accent.gamma_multiply(0.16), theme.accent, gh));
        icons::draw(p, Icon::PlaySolid, icons::center_rect(bc + vec2(0.8, 0.0), 13.0), lerp_color(theme.accent, theme.bar, gh));
    }

    let er = Rect::from_center_size(pos2(r.max.x - 20.0, r.center().y), vec2(26.0, 26.0));
    let edit = ui.interact(er, id.with("edit"), CLICK);
    if edit.hovered() {
        p.rect_filled(er, 7.0, theme.header_focused);
    }
    icons::draw(p, Icon::Pencil, icons::center_rect(er.center(), 13.0), lerp_color(theme.muted, theme.text, f32::from(edit.hovered())).gamma_multiply(0.3 + 0.7 * hover));

    let x = r.min.x + 48.0;
    let clip = p.with_clip_rect(Rect::from_min_max(r.min, pos2(er.min.x - 4.0, r.max.y)));
    clip.text(pos2(x, r.min.y + 16.5), Align2::LEFT_CENTER, &l.name, FontId::proportional(13.5), theme.text);
    let missing = expand(&l.dir).is_some_and(|d| !d.is_dir());
    let (sub, col) = match status {
        Some(Status { state: State::Starting, tab }) => (format!("starting in tab {tab}…"), theme.accent),
        Some(Status { state: State::Running(fg), tab }) => (format!("● running {fg} · tab {tab}"), green),
        Some(Status { state: State::Finished, tab }) => (format!("finished · tab {tab}"), theme.muted),
        None if missing => (format!("folder not found: {}", l.dir.trim()), red),
        None => (steps(l).join("  →  "), theme.muted),
    };
    clip.text(pos2(x, r.min.y + 33.0), Align2::LEFT_CENTER, sub, FontId::monospace(10.5), col);

    let tip = if busy { "Stop (Ctrl+C)" } else if status.is_some() { "Run again in its tab" } else { "Run" };
    if go.on_hover_text(tip).clicked() {
        actions.push(UiAction::Launcher(l.id, if busy { Op::Stop } else { Op::Run }));
    }
    if edit.on_hover_text("Edit").clicked() {
        change = Some(Change::Edit(l.id));
    }
    let resp = resp.on_hover_ui(|ui| {
        ui.label(RichText::new(&l.name).strong());
        let folder = if l.dir.trim().is_empty() { "the focused pane's folder" } else { l.dir.trim() };
        ui.label(RichText::new(format!("in {folder}")).color(theme.muted));
        for (k, s) in steps(l).iter().enumerate() {
            ui.label(RichText::new(format!("{}. {s}", k + 1)).monospace());
        }
    });
    if resp.clicked() && status.is_some() {
        actions.push(UiAction::Launcher(l.id, Op::Show));
    }
    resp.context_menu(|ui| {
        let item = |ui: &mut egui::Ui, enabled: bool, label: &str| {
            let hit = ui.add_enabled(enabled, egui::Button::new(label)).clicked();
            if hit {
                ui.close_menu();
            }
            hit
        };
        if busy {
            if item(ui, true, "Stop (Ctrl+C)") {
                actions.push(UiAction::Launcher(l.id, Op::Stop));
            }
            if item(ui, true, "Restart") {
                actions.push(UiAction::Launcher(l.id, Op::Restart));
            }
        } else if item(ui, true, "Run") {
            actions.push(UiAction::Launcher(l.id, Op::Run));
        }
        if status.is_some() {
            if item(ui, true, "Go to its tab") {
                actions.push(UiAction::Launcher(l.id, Op::Show));
            }
            if item(ui, true, "Close its pane") {
                actions.push(UiAction::Launcher(l.id, Op::Close));
            }
        }
        ui.separator();
        if item(ui, true, "Edit…") {
            change = Some(Change::Edit(l.id));
        }
        if item(ui, true, "Duplicate") {
            change = Some(Change::Duplicate(l.id));
        }
        if item(ui, true, "Copy command line") {
            actions.push(UiAction::Copy(command_line(l, true)));
        }
        if item(ui, i > 0, "Move up") {
            change = Some(Change::Move(l.id, -1));
        }
        if item(ui, i + 1 < n, "Move down") {
            change = Some(Change::Move(l.id, 1));
        }
        ui.separator();
        if item(ui, true, "Delete") {
            change = Some(Change::Delete(l.id));
        }
    });
    change
}

/// The launcher editor card: name, folder, commands, where to run them.
fn editor(ui: &mut egui::Ui, theme: &Theme, st: &mut LaunchpadUi) -> Option<EditOut> {
    let d = st.draft.as_mut()?;
    let ids = ["name", "dir", "cmds"].map(|k| Id::new(("s-term-launch-edit", k)));
    let mut out = None;
    let mut browse = false;
    crate::usage_ui::card(ui, theme, |ui| {
        let title = if d.id.is_some() { "Edit launcher" } else { "New launcher" };
        ui.label(RichText::new(title).size(13.0).strong().color(theme.text));
        ui.add_space(6.0);
        let label = |ui: &mut egui::Ui, text: &str| {
            ui.add_space(4.0);
            ui.label(RichText::new(text).size(11.0).color(theme.muted));
        };
        label(ui, "Name");
        let name = ui.add(egui::TextEdit::singleline(&mut d.name).id(ids[0]).hint_text("my-app").desired_width(f32::INFINITY));
        label(ui, "Folder");
        // Right to left: the button takes its size and the field gets exactly the rest (a field
        // sized by guessing the button's width overflows, and the side panel grows to fit it).
        let dir = ui
            .horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    browse = ui.add(egui::Button::new("Browse…").sense(CLICK)).clicked();
                    let field = egui::TextEdit::singleline(&mut d.dir).id(ids[1]).hint_text("the focused pane's").font(egui::TextStyle::Monospace);
                    ui.add(field.desired_width(ui.available_width()))
                })
                .inner
            })
            .inner;
        let folder = expand(&d.dir);
        if folder.as_ref().is_some_and(|f| !f.is_dir()) {
            ui.label(RichText::new("Not a folder").size(10.5).color(c32(theme.palette[1])));
        }
        label(ui, "Commands, one per line");
        let cmds = ui.add(
            egui::TextEdit::multiline(&mut d.commands).id(ids[2]).font(egui::TextStyle::Monospace).desired_rows(4).desired_width(f32::INFINITY),
        );
        ui.label(RichText::new("Each one runs only if the one before it succeeded.").size(10.5).color(theme.muted));
        label(ui, "Run in");
        crate::app::segmented(ui, theme, &mut d.run_in, &[(RunIn::Tab, "New tab"), (RunIn::Split, "Split"), (RunIn::Here, "This pane")], "s-term-launch-run-in");
        let where_ = match d.run_in {
            RunIn::Tab => "Opens a new tab in the folder.",
            RunIn::Split => "Opens a pane to the right of the focused one.",
            RunIn::Here => "Types into the focused pane when it's idle at a prompt; otherwise opens a new tab.",
        };
        ui.label(RichText::new(where_).size(10.5).color(theme.muted));

        let l = d.launcher();
        let valid = !steps(&l).is_empty();
        // What gets typed: a new pane already starts in the folder, so it needs no `cd`.
        let cd = !(folder.as_ref().is_some_and(|f| f.is_dir()) && d.run_in != RunIn::Here);
        if valid {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("❯ {}", command_line(&l, cd))).monospace().size(10.5).color(theme.accent2));
        }

        if std::mem::take(&mut d.focus) {
            // The name usually comes filled in from the folder; then the commands are what's left to type.
            ui.memory_mut(|m| m.request_focus(if d.name.is_empty() { ids[0] } else { ids[2] }));
        }
        // Enter in a one-line field moves on to the next field (egui would just drop the focus).
        let (enter, cmd_enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.modifiers.command && i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if enter && !cmd_enter && name.lost_focus() {
            ui.memory_mut(|m| m.request_focus(ids[1]));
        }
        if enter && !cmd_enter && dir.lost_focus() {
            ui.memory_mut(|m| m.request_focus(ids[2]));
        }
        // Only keys meant for the editor: Esc in a pane must not cancel it.
        let mine = [&name, &dir, &cmds].iter().any(|r| r.has_focus() || r.lost_focus());

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let save = egui::Button::new(RichText::new("Save").color(if valid { theme.bar } else { theme.muted })).fill(if valid { theme.accent } else { theme.header }).sense(CLICK);
            if ui.add_enabled(valid, save).on_disabled_hover_text("Add a command first").clicked() || (valid && mine && cmd_enter) {
                out = Some(EditOut::Save(l.clone()));
            }
            if ui.add(egui::Button::new("Cancel").sense(CLICK)).clicked() || (mine && esc) {
                out = Some(EditOut::Cancel);
            }
            if let Some(id) = d.id {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("Delete").color(c32(theme.palette[1]))).frame(false).sense(CLICK)).clicked() {
                        out = Some(EditOut::Delete(id));
                    }
                });
            }
        });
        ui.label(RichText::new("Ctrl+Enter saves · Esc cancels").size(10.0).color(theme.muted.gamma_multiply(0.8)));
    });
    // Read from memory, not the responses: a focus requested this frame already counts.
    st.typing = out.is_none() && ui.memory(|m| ids.iter().any(|id| m.has_focus(*id)));
    if browse {
        crate::sidebar::pick_folder("Launcher folder", st.picked.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launcher(dir: &str, commands: &[&str]) -> Launcher {
        Launcher { id: 1, name: "x".into(), dir: dir.into(), commands: commands.iter().map(|s| s.to_string()).collect(), run_in: RunIn::Tab }
    }

    #[test]
    fn command_line_joins_steps() {
        let l = launcher("/srv/my app", &["npm install", "", "  # comment", "npm run dev  "]);
        assert_eq!(steps(&l), ["npm install", "npm run dev"]);
        assert_eq!(command_line(&l, true), "cd '/srv/my app' && npm install && npm run dev");
        assert_eq!(command_line(&l, false), "npm install && npm run dev");
        assert_eq!(command_line(&launcher("", &["make"]), true), "make");
    }

    #[test]
    fn home_folder_shorthand() {
        let Some(h) = home() else { return };
        assert_eq!(expand("~/web"), Some(h.join("web")));
        assert_eq!(expand(" ~ "), Some(h.clone()));
        assert_eq!(expand("~other/x"), Some(PathBuf::from("~other/x")));
        assert_eq!(expand("  "), None);
        assert_eq!(tilde(&h.join("a/b")), "~/a/b");
        assert_eq!(tilde(Path::new("/srv")), "/srv");
    }

    #[test]
    fn new_draft_takes_the_folder_but_no_commands() {
        let dir = std::env::temp_dir().join(format!("s-term-launch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"dev":"astro dev"}}"#).unwrap();
        let mut ui = LaunchpadUi::default();
        ui.new_draft(Some(&dir));
        let d = ui.draft.as_ref().unwrap();
        assert_eq!(d.name, dir.file_name().unwrap().to_string_lossy());
        assert_eq!(expand(&d.dir), Some(dir.clone()));
        assert!(d.commands.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn draft_names_itself_after_the_folder() {
        let d = Draft { id: None, name: " ".into(), dir: "/srv/shop/".into(), commands: "make\n\n  \nmake test".into(), run_in: RunIn::Split, focus: false };
        let l = d.launcher();
        assert_eq!((l.name.as_str(), l.dir.as_str(), l.run_in), ("shop", "/srv/shop/", RunIn::Split));
        assert_eq!(l.commands, ["make", "make test"]);
        let cfg = Config { launchers: vec![Launcher { id: u64::MAX - 1, ..l }], ..Config::default() };
        assert_eq!(new_id(&cfg), u64::MAX);
    }

    /// Drives real egui frames: the play button asks to run, and to stop once it's running.
    #[test]
    fn play_button_runs_and_stops() {
        use egui::{Event, PointerButton, RawInput};
        let ctx = egui::Context::default();
        let theme = Theme::preset("graphite");
        let mut cfg = Config { launchers: vec![launcher("", &["sleep 5"])], ..Config::default() };
        let mut st = LaunchpadUi::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        let mut frame = |events: Vec<Event>, runs: &Statuses| {
            let mut actions = Vec::new();
            let input = RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| panel(ui, &theme, &mut st, &mut cfg, runs, None, &mut actions));
            });
            actions
        };
        let idle = Statuses::new();
        frame(vec![], &idle);
        let btn = ctx.read_response(Id::new(("s-term-launch", 1u64)).with("go")).expect("play button").rect.center();
        let click = |pressed| Event::PointerButton { pos: btn, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        let ops = |a: Vec<UiAction>| a.into_iter().filter_map(|a| if let UiAction::Launcher(id, op) = a { Some((id, op)) } else { None }).collect::<Vec<_>>();
        frame(vec![Event::PointerMoved(btn)], &idle);
        frame(vec![click(true)], &idle);
        assert_eq!(ops(frame(vec![click(false)], &idle)), [(1, Op::Run)]);

        let running = Statuses::from([(1, Status { state: State::Running("sleep".into()), tab: 2 })]);
        frame(vec![], &running);
        frame(vec![click(true)], &running);
        assert_eq!(ops(frame(vec![click(false)], &running)), [(1, Op::Stop)]);
    }

    /// In a resizable panel, anything wider than the room it's given makes the panel grow each frame.
    #[test]
    fn editor_keeps_the_panel_width() {
        let ctx = egui::Context::default();
        let theme = Theme::preset("graphite");
        let mut cfg = Config { launchers: vec![launcher("/srv", &["make"])], ..Config::default() };
        let mut st = LaunchpadUi::default();
        st.new_draft(Some(Path::new("/srv/some-project")));
        ctx.style_mut(|s| s.spacing.button_padding = vec2(10.0, 5.0)); // as in the app (wider buttons)
        let mut width = 0.0;
        for _ in 0..20 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))), ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                let r = egui::SidePanel::left("t").resizable(true).default_width(340.0).width_range(240.0..=900.0).show(ctx, |ui| {
                    panel(ui, &theme, &mut st, &mut cfg, &Statuses::new(), None, &mut Vec::new());
                });
                width = r.response.rect.width();
            });
        }
        assert!(width < 345.0, "panel grew to {width}");
    }

    /// The editor owns the keyboard from its first frame, so the first key typed doesn't leak to the shell.
    #[test]
    fn editor_takes_the_keyboard_at_once() {
        let ctx = egui::Context::default();
        let theme = Theme::preset("graphite");
        let mut cfg = Config::default();
        let mut st = LaunchpadUi::default();
        st.new_draft(None);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| panel(ui, &theme, &mut st, &mut cfg, &Statuses::new(), None, &mut Vec::new()));
        });
        assert!(st.typing);
    }
}

//! First-run setup: which AI tools to track, and the provider for "Ask S-term".
//! Shown once (until `setup_done`), and again from Settings → About → Run setup.

use egui::{pos2, vec2, Color32, Id, RichText, Sense};

use super::settings::{row, switch};
use super::STerm;
use crate::theme::Theme;

#[derive(Default)]
pub struct Setup {
    step: usize,
}

const STEPS: usize = 3;

impl Setup {
    /// Opens at a given step (for screenshots).
    pub fn at(step: usize) -> Self {
        Self { step: step.min(STEPS - 1) }
    }
}

fn home() -> std::path::PathBuf {
    std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default()
}

fn found(dir: std::path::PathBuf) -> bool {
    dir.is_dir()
}

/// (Claude Code, Codex, Antigravity) data folders present on this machine.
fn installed() -> (bool, bool, bool) {
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| home().join(".claude"));
    let codex = std::env::var_os("CODEX_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home().join(".codex"));
    (found(claude), found(codex), found(home().join(".gemini/antigravity-cli")))
}

impl STerm {
    pub(super) fn setup_ui(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.setup.take() else { return };
        let (has_claude, has_codex, has_agy) = installed();
        let theme = Theme::preset(&self.config.theme);
        let screen = ctx.screen_rect();
        egui::Area::new(Id::new("s-term-setup-backdrop")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, _) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(150));
        });
        let width = (screen.width() - 40.0).min(600.0);
        let pos = pos2(screen.center().x - width / 2.0, (screen.center().y - 270.0).max(screen.min.y + 20.0));
        let mut done = false;
        let mut cfg = self.config.clone();
        egui::Area::new(Id::new("s-term-setup")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            crate::glass::frame(ui, &theme, 22.0, theme.panel, crate::glass::Lift::High, 26.0, |ui| {
                ui.set_width(width - 52.0);
                // ---- progress dots
                ui.horizontal(|ui| {
                    for i in 0..STEPS {
                        let (r, _) = ui.allocate_exact_size(vec2(if i == st.step { 22.0 } else { 8.0 }, 8.0), Sense::hover());
                        ui.painter().rect_filled(r, 4.0, if i <= st.step { theme.accent } else { theme.border });
                    }
                });
                ui.add_space(14.0);
                match st.step {
                    0 => {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
                            ui.painter().rect_filled(r, 14.0, theme.bar);
                            super::chrome::paint_logo(ui.painter(), self.logo.id(), r.shrink(6.0), &theme);
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Welcome to S-term").size(22.0).strong().color(theme.text));
                                ui.label(RichText::new("A modern terminal, written in Rust.").size(13.0).color(theme.muted));
                            });
                        });
                        ui.add_space(14.0);
                        for (title, desc) in [
                            ("Splits and tabs", "Ctrl+Shift+E splits to the right, Ctrl+Shift+O splits down, Ctrl+Shift+T opens a tab."),
                            ("Your repo at a glance", "Ctrl+Shift+B opens the side panel: changes, diffs and the file tree of the folder you're in."),
                            ("AI usage", "Context, plan limits and tokens for the AI coding tools you use."),
                            ("Ask S-term", "An assistant in the top bar that can read your panes, open new ones and run commands for you."),
                        ] {
                            ui.add_space(6.0);
                            ui.label(RichText::new(title).size(13.5).strong().color(theme.text));
                            ui.label(RichText::new(desc).size(12.0).color(theme.muted));
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new("Two quick choices and you're set. You can change both later in Settings.").size(12.0).color(theme.muted));
                    }
                    1 => {
                        ui.label(RichText::new("Which AI tools do you use?").size(20.0).strong().color(theme.text));
                        ui.label(RichText::new("S-term shows their usage in the side panel and status bar. Everything is read locally.").size(12.5).color(theme.muted));
                        ui.add_space(6.0);
                        let tag = |f: bool| if f { "Found on this computer." } else { "Not found on this computer." };
                        row(ui, &theme, "Claude Code", &format!("Context, 5-hour and weekly limits, tokens per model. {}", tag(has_claude)), |ui| {
                            switch(ui, &theme, &mut cfg.show_claude, Id::new("setup-claude"));
                        });
                        row(ui, &theme, "Codex CLI", &format!("Context and tokens per model. {}", tag(has_codex)), |ui| {
                            switch(ui, &theme, &mut cfg.show_codex, Id::new("setup-codex"));
                        });
                        row(ui, &theme, "Antigravity CLI", &format!("The current conversation and tokens per model. {}", tag(has_agy)), |ui| {
                            switch(ui, &theme, &mut cfg.show_antigravity, Id::new("setup-agy"));
                        });
                    }
                    _ => {
                        ui.label(RichText::new("Ask S-term").size(20.0).strong().color(theme.text));
                        ui.label(
                            RichText::new("Pick a provider and add an API key to use the assistant in the top bar. It's optional: skip this and add a key later in Settings → Assistant.")
                                .size(12.5)
                                .color(theme.muted),
                        );
                        ui.add_space(12.0);
                        super::ask::key_editor(ui, &theme, &mut cfg, &mut self.ask);
                        ui.add_space(4.0);
                        row(ui, &theme, "Run commands without asking", "Off: you approve each command first. Recommended.", |ui| {
                            switch(ui, &theme, &mut cfg.ai_auto_run, Id::new("setup-autorun"));
                        });
                    }
                }
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    if st.step > 0 && ui.button("Back").clicked() {
                        st.step -= 1;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let last = st.step + 1 == STEPS;
                        let label = if last { "Finish" } else if st.step == 0 { "Get started" } else { "Next" };
                        if ui.add(egui::Button::new(RichText::new(label).color(theme.pick_ink)).fill(theme.accent).min_size(vec2(96.0, 30.0))).clicked() {
                            if last {
                                done = true;
                            } else {
                                st.step += 1;
                            }
                        }
                        if !last && ui.button("Skip setup").clicked() {
                            done = true;
                        }
                    });
                });
            });
        });
        if done {
            cfg.setup_done = true;
        } else {
            self.setup = Some(st);
        }
        self.config = cfg;
    }
}

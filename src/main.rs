//! S-term — a modern terminal, written in Rust.
//!
//! Module map (each one is a seam for future features):
//! - `app`    : window, tabs, pane tree, input dispatch (+ chrome, panes, palette, ask, settings, setup)
//! - `assistant`: "Ask S-term" (providers, API keys, the tool-calling loop)
//! - `config` : user settings (~/.config/s-term/config.json)
//! - `history`: closed tabs and last sessions (~/.local/share/s-term/history.json) + the History view
//! - `icons`  : vector line icons
//! - `pane`   : one terminal session (PTY + alacritty_terminal state)
//! - `layout` : the split tree
//! - `render` : painting a terminal grid with egui
//! - `input`  : key -> action / key -> bytes translation
//! - `sidebar`: git changes / file tree panel + diff viewer
//! - `theme`  : colors
//! - `usage`  : Claude Code / Codex / Antigravity token usage + `s-term statusline`
//! - `usage_ui`: the AI usage tab and tab-bar meter

mod app;
mod assistant;
mod config;
mod fonts;
mod glass;
mod history;
mod icons;
mod input;
mod launchpad;
mod layout;
mod pane;
mod peers;
mod pricing;
mod render;
mod sidebar;
mod theme;
mod usage;
mod usage_ui;

fn main() -> eframe::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("statusline") {
        // Claude Code status-line hook: record usage for S-term, print a short line, exit.
        let _ = usage::statusline_main();
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("usage") {
        usage::report_main();
        return Ok(());
    }
    let config = config::Config::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("S-term")
            .with_decorations(config.system_titlebar)
            // Frameless windows draw their own rounded corners, so the surface needs an alpha channel.
            .with_transparent(!config.system_titlebar)
            .with_app_id("s-term")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([320.0, 200.0])
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: include_bytes!("../assets/icon/app-64.rgba").to_vec(),
                width: 64,
                height: 64,
            })),
        ..Default::default()
    };

    eframe::run_native(
        "S-term",
        options,
        Box::new(move |cc| Ok(Box::new(app::STerm::new(cc, config)))),
    )
}

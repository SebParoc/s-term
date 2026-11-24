//! User settings, saved as JSON in `~/.config/s-term/config.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum PaneHeaders {
    Always,
    Splits,
    Never,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum HeatMetric {
    Output,
    Total,
}

/// What goes before the `❯` in the shell prompt (see `scripts/s-term-prompt.sh`).
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum PromptPrefix {
    /// Just the arrow.
    None,
    /// `~/s-term ❯`
    Folder,
    /// `user@host:~/s-term ❯`
    HostFolder,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum CursorStyle {
    /// Whatever the running program asks for.
    App,
    Block,
    Beam,
    Underline,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    /// Empty = pick the best installed coding font.
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub cursor_glide: bool,
    pub animations: bool,
    pub pane_headers: PaneHeaders,
    pub dim_inactive: bool,
    pub system_titlebar: bool,
    pub status_bar: bool,
    pub copy_on_select: bool,
    pub scrollback: usize,
    /// Empty = `$SHELL`.
    pub shell: String,
    pub show_claude: bool,
    pub show_antigravity: bool,
    pub heat_metric: HeatMetric,
    /// Ctrl+= / Ctrl+- / Ctrl+wheel scale the whole interface instead of the terminal text.
    pub zoom_ui: bool,
    /// Interface scale when `zoom_ui` is on.
    pub ui_scale: f32,
    pub prompt: PromptPrefix,
    pub cursor_style: CursorStyle,
    /// Mouse-wheel scrollback speed multiplier.
    pub scroll_speed: f32,
    /// New tabs start in the home folder instead of the focused pane's folder.
    pub new_tab_home: bool,
    /// Extra folders shown in the Explorer, stacked under the followed folder.
    pub explorer_folders: Vec<PathBuf>,
    /// Explorer shows dotfiles.
    pub show_hidden: bool,
    /// Explorer leaves out git-ignored files instead of dimming them.
    pub hide_ignored: bool,
    /// Open the side panel when S-term starts.
    pub panel_on_start: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "neon".into(),
            font_family: String::new(),
            font_size: 14.0,
            line_height: 1.08,
            cursor_glide: true,
            animations: true,
            pane_headers: PaneHeaders::Splits,
            dim_inactive: true,
            system_titlebar: false,
            status_bar: true,
            copy_on_select: false,
            scrollback: 10_000,
            shell: String::new(),
            show_claude: true,
            show_antigravity: true,
            heat_metric: HeatMetric::Output,
            zoom_ui: false,
            ui_scale: 1.0,
            prompt: PromptPrefix::None,
            cursor_style: CursorStyle::App,
            scroll_speed: 1.0,
            new_tab_home: false,
            explorer_folders: Vec::new(),
            show_hidden: true,
            hide_ignored: false,
            panel_on_start: false,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("s-term/config.json")
    }

    pub fn load() -> Self {
        let raw: Option<serde_json::Value> = std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let mut c: Config = raw.clone().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        if let Some(v) = raw.filter(|v| v.get("prompt").is_none()) {
            c.prompt = Self::legacy_prompt(&v);
        }
        c.font_size = c.font_size.clamp(6.0, 48.0);
        c.line_height = c.line_height.clamp(0.8, 2.0);
        c.ui_scale = c.ui_scale.clamp(0.5, 2.5);
        c.scrollback = c.scrollback.min(1_000_000);
        c.scroll_speed = c.scroll_speed.clamp(0.25, 5.0);
        c
    }

    /// Older configs had two switches, `prompt_dir` and `prompt_user`.
    fn legacy_prompt(v: &serde_json::Value) -> PromptPrefix {
        let on = |k: &str| v.get(k).and_then(|b| b.as_bool()).unwrap_or(false);
        match (on("prompt_dir"), on("prompt_user")) {
            (_, true) => PromptPrefix::HostFolder,
            (true, false) => PromptPrefix::Folder,
            (false, false) => PromptPrefix::None,
        }
    }

    /// The shell prompt snippet shows the folder / `user@host` while these files exist, so the
    /// setting reaches every open pane at its next prompt.
    pub fn write_prompt_flags(&self) {
        let dir = self.prompt != PromptPrefix::None;
        let user = self.prompt == PromptPrefix::HostFolder;
        for (name, on) in [("prompt-dir", dir), ("prompt-user", user)] {
            let p = Self::path().with_file_name(name);
            if on {
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(p, b"");
            } else {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_fills_defaults() {
        let c: Config = serde_json::from_str(r#"{"theme":"ember","font_size":16}"#).unwrap();
        assert_eq!(c.theme, "ember");
        assert_eq!(c.font_size, 16.0);
        assert!(c.cursor_glide);
        assert_eq!(c.pane_headers, PaneHeaders::Splits);
        assert_eq!(c.prompt, PromptPrefix::None);
        assert!(c.explorer_folders.is_empty());
    }

    #[test]
    fn legacy_prompt_switches_map_to_prefix() {
        let v = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(Config::legacy_prompt(&v(r#"{}"#)), PromptPrefix::None);
        assert_eq!(Config::legacy_prompt(&v(r#"{"prompt_dir":true}"#)), PromptPrefix::Folder);
        assert_eq!(Config::legacy_prompt(&v(r#"{"prompt_user":true}"#)), PromptPrefix::HostFolder);
        assert_eq!(Config::legacy_prompt(&v(r#"{"prompt_dir":true,"prompt_user":true}"#)), PromptPrefix::HostFolder);
    }

    #[test]
    fn explorer_folders_round_trip() {
        let c = Config { explorer_folders: vec![PathBuf::from("/tmp/a"), PathBuf::from("/srv/b")], ..Config::default() };
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.explorer_folders, c.explorer_folders);
    }
}

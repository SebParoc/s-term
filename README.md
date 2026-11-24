# ⚡ S-term

A terminal for vibe coding, written in Rust. It has Terminator-style splits and tabs.

Built on [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal) for the VT emulation, scrollback and PTY,
and [`egui`/`eframe`](https://github.com/emilk/egui) for the window and GPU rendering.

## Build & run

```sh
cargo run --release
```

## Keybindings (Terminator-style)

| Keys | Action |
|---|---|
| `Ctrl+Shift+O` | Split horizontally (panes stacked top/bottom) |
| `Ctrl+Shift+E` | Split vertically (panes side by side) |
| `Ctrl+Shift+W` | Close pane |
| `Ctrl+Shift+T` | New tab |
| `Ctrl+PageDown` / `Ctrl+Tab` | Next tab |
| `Ctrl+PageUp` / `Ctrl+Shift+Tab` | Previous tab |
| `Alt+←/→/↑/↓` | Move focus to the neighboring pane |
| `Ctrl+Shift+N` | New window (opens in the focused pane's folder) |
| `Ctrl+Shift+P` | Command palette: every action, view, theme and toggle, with fuzzy search |
| `Ctrl+,` | Settings |
| `Ctrl+Shift+←/→/↑/↓` | Resize pane |
| `Ctrl+Shift+X` | Maximize / restore pane |
| `Alt+A` / `Alt+O` | Broadcast typing to all panes in tab / stop broadcasting |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste |
| `Shift+PageUp/PageDown/Home/End` | Scroll back |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0`, or `Ctrl` + mouse wheel | Zoom in / out / reset. Zooms the terminal text, or the whole interface if Settings → Zoom scales is set to that |
| `Shift+Enter` | Newline without submitting (sends `ESC CR`, which Claude Code and similar tools understand) |
| `Ctrl+Shift+B` | Toggle the side panel (Changes / Files) |
| `Esc` | Close the diff or file viewer |
| `Ctrl+Shift+Q` | Quit |

Mouse: click to focus a pane, drag to select, double-click for a word, triple-click for a line, drag dividers to resize,
scroll wheel for scrollback, middle-click a tab to close it.

## Interface

- **Title bar**: S-term draws its own. It holds the tabs (drag to reorder, middle-click to close), a command search
  box, a usage meter, settings and a close button. Drag empty space to move the window and double-click to
  maximize. To use your desktop's title bar instead, turn on Settings → Appearance → System title bar.
- **Activity rail** on the left: Explorer, Source Control (with a count of changed files) and AI Usage (with a
  context-level dot). Click an icon to open that view; click it again to close.
- **Status bar**: branch, changes, folder, Claude context and limits, pane count, grid size, theme and font.
  Click an item to jump to the matching view.
- **Settings** (`Ctrl+,`): themes (Neon, Midnight, Ember, Aurora, Graphite, Atelier), font family, font size, line height,
  animations, cursor glide, pane title bars, dimming, status bar, shell, scrollback, where new tabs open, folder and user@host in
  the prompt, cursor style, scroll speed, copy-on-select, Explorer folders and filters, and the AI view.
  Changes apply as you make them and are saved to `~/.config/s-term/config.json`.
- **Activity heatmap** in AI Usage: a GitHub-style calendar of daily tokens. With "All models", each day takes the
  color of its busiest model. Click a model to see only that model, and hover a day for its breakdown. S-term keeps
  its own daily log in `~/.local/share/s-term/usage-daily.json`, so history survives Claude Code's 30-day
  transcript cleanup.

## Side panel

`Ctrl+Shift+B`, or an icon on the activity rail, opens a panel that follows the folder of the focused pane:

- **Changes**: git status for the repository, most recently touched files first, with +/− line counts and how long ago
  each file changed. Files changed in the last 90 seconds get an accent dot. Click a file to see its diff.
  Recent commits are listed below.
- **Files**: the project tree. Changed files are colored and ignored files are dimmed (or hidden, see Settings →
  Explorer), and images get a picture icon. Click a file to preview it; images (PNG, JPEG, GIF, WebP, BMP, ICO) are
  shown as pictures.
- **More folders**: the **+** in the Explorer header (or right-click a folder → *Add to Explorer as a section*, or
  Settings → Explorer) adds another folder. The panel splits into stacked sections that share its height; click a
  section's header to fold it, **×** to remove it. The list is saved in the config file.
- **Drag** a file from the panel, or from your file manager, onto any pane to paste its absolute path there. Claude Code
  turns a pasted image path into an attachment.
- **Middle-click** a file to type its path into the terminal, which is handy for pointing Claude Code at it.
  **Right-click** a file to copy its path or open it in the default app.

- **✦ AI**: one flat page in four sections. **Now** shows each tool's live session with a context ring. **Plan limits**
  shows Claude Code's 5-hour and weekly limits. **Usage** gives one total and one list of models across **Claude Code**
  and the **Antigravity CLI** for the last 5 hours, today or the last 7 days (hover a model for its token breakdown),
  plus a 24-hour output chart. **Activity** is the heatmap. The live session in the focused pane's folder gets a context ring that turns yellow at 50% and red at 75%.
  Claude Code's 5-hour and weekly plan limits appear here too, with reset countdowns.
  The tab bar also shows a small meter with the focused pane's context and the 5-hour limit; click it to open this tab.

It refreshes about once a second on a background thread and never takes git's index lock, so it won't get in the way
of git commands an agent is running.

### Claude Code plan limits

Claude Code only shares its 5-hour and weekly limits, and the exact context-window size, with its status-line
command. Point that at S-term in `~/.claude/settings.json`:

```json
"statusLine": { "type": "command", "command": "~/.local/bin/s-term statusline" }
```

`s-term statusline` saves what Claude Code sends to `~/.cache/s-term/claude-status/` and prints a short line
(`Opus 5.5 · ctx 42% · 5h 38% · week 12%`). `s-term usage` prints the same totals the AI tab shows.

Token counts come from Claude Code's transcripts in `~/.claude/projects`, and from Antigravity's conversation databases
in `~/.gemini/antigravity-cli/conversations`, which S-term opens read-only. Nothing leaves your machine.

## Code map

- `src/app.rs`: tabs, the pane tree, input dispatch and the frame layout
  - `src/app/chrome.rs`: title bar, activity rail, status bar and window resizing
  - `src/app/panes.rs`: panes (layout animation, mouse and painting)
  - `src/app/palette.rs`: command palette
  - `src/app/settings.rs`: settings page
  - `src/app/devshot.rs`: `S_TERM_SCREENSHOT=out.png S_TERM_DEMO=ai,split` saves a screenshot and exits
- `src/config.rs`: settings file
- `src/icons.rs`: vector line icons
- `src/pane.rs`: one shell session (PTY plus emulator state)
- `src/layout.rs`: the split tree
- `src/render.rs`: grid painting
- `src/input.rs`: keybindings, and keys to escape sequences
- `src/sidebar.rs`: the side panel (git and file data on a background thread) and the diff/file viewer
- `src/usage.rs`: Claude Code and Antigravity usage readers, `s-term statusline` and `s-term usage`
- `src/usage_ui.rs`: the AI tab, the icons and the tab-bar meter
- `src/theme.rs`: color themes (including the warm "Atelier")
- `src/glass.rs`: glass surfaces (sheen, rim light, shadow), the warm backdrop, the sliding selection pill and the search orb

## Prompt

`scripts/install.sh` copies `scripts/s-term-prompt.sh` to `~/.bashrc.d/`. Inside S-term it makes the bash prompt a
plain `❯` (red after a failed command), since the pane header already shows the folder. Settings → Terminal →
*Before the ❯* picks what goes in front: nothing, the folder (`~/s-term ❯`), or user@host and the folder
(`user@host:~/s-term ❯`). It takes effect at the next prompt in every pane.

## Fonts & icon

S-term ships **JetBrains Mono** for the terminal and **Inter** for the interface, both under the SIL Open Font
License (`assets/fonts/`, licenses included). Any installed monospace font can be picked in Settings. The scorpion
"S" app icon is in `assets/icon/`; `scripts/install.sh` installs it for the desktop and dock.

## Ideas for later

Config file and themes, custom fonts with real bold and italic, search, clickable URLs, mouse reporting for TUIs,
an AI side panel, session save and restore, pane groups, and a quake-style drop-down.

## Install into the app menu

```sh
./scripts/install.sh
```

This links `~/.local/bin/s-term` to `target/release/s-term`. After that, every `cargo build --release` updates the
installed app; just reopen S-term.

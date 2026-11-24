# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

S-term is a Linux terminal emulator in Rust: `alacritty_terminal` does emulation, scrollback and the PTY, and
`egui`/`eframe` 0.29 does the window and rendering. README.md is the user-facing manual. It has the keyboard
shortcut table and a module-by-module **Code map**, so keep both up to date when you add bindings or modules.

## Commands

```sh
cargo build --release          # the installed app runs target/release/s-term, so build this to see UI changes
cargo run --release
cargo test                     # unit tests live in #[cfg(test)] blocks at the bottom of each module
cargo test calendar            # one test, by name substring (or a full path: usage::tests::calendar)
cargo test -- --ignored live_  # live tests that call the real Anthropic/OpenAI/Gemini APIs (needs network)
cargo clippy                   # about a dozen warnings already exist; it isn't a gate
./scripts/install.sh           # symlinks ~/.local/bin/s-term -> target/release/s-term, plus icons, desktop entry and prompt script
./scripts/package.sh           # release tarball in dist/ (the same install.sh, run from the tarball, copies the prebuilt binary)
```

Releases: bump `version` in `Cargo.toml` and push a `v*` tag. `.github/workflows/release.yml` builds the tarball in
an Ubuntu 22.04 container (so the binary needs only glibc 2.35, while a local Fedora build needs 2.43) and attaches it
to a GitHub Release. `install.sh` detects its mode: `Cargo.toml` next to it means a clone (build and symlink), a
`s-term` binary means a release (copy). Keep both modes working when you change it.

- **Don't run `cargo fmt`.** The code isn't rustfmt-formatted. It uses wide lines (up to ~270 columns) and compact
  one-line expressions, and rustfmt would rewrite almost every file. Match the style around your change.
- Dependencies build at `opt-level = 2` even in debug (`[profile.dev.package."*"]`), so debug builds run at a
  usable speed.
- The binary has two subcommands besides the GUI: `s-term statusline`, the Claude Code status-line hook, and
  `s-term usage`, which prints usage totals. Both are handled in `main.rs` before eframe starts.

### Checking UI changes visually

Screenshot mode (`src/app/devshot.rs`) opens the real window, applies demo steps, saves a PNG at frame 90 and exits.
It never saves the user's config:

```sh
S_TERM_SCREENSHOT=target/shots/x.png S_TERM_DEMO=split,ai ./target/release/s-term
```

Steps, comma-separated: `ai` `changes` `files` `settings` `section=<name>` `palette` `commands` `ask` (a sample
conversation waiting for approval) `asktyped=<text>` `setup` `setup=<n>` `split` `splitdown` `tab` `zoom`
`theme=<id>` `folder=<dir>` `view=<file>` `wait=<s>` (shoot about s seconds later, e.g. for usage data) `launch` (the Launchpad view) `launchdemo` (load sample launchers)
`launchnew` (the editor, filled in from the focused pane's folder) `launchrun` (run the sample ping; after
`launchdemo`) `history` (the History view) `historydemo` (sample History entries) `restoreoffer` (the startup
"reopen your last session?" popup). Open the PNG with the Read tool to check the result. `S_TERM_FONT="Family"`
forces a font.

## Architecture

### One immediate-mode frame loop

All app state is one struct, `STerm` in `src/app.rs`. `eframe::App::update` is the whole per-frame pipeline, in
this order: apply config changes, cell metrics, drain terminal events, keyboard, title bar, status bar, rail, side
panel, panes, viewer, settings, assistant tick, palette, setup, overlays. Each file in `src/app/` (`chrome`, `panes`,
`palette`, `ask`, `settings`, `setup`, `devshot`) is another `impl STerm` block for one area of the UI, with
`pub(super)` methods. They aren't separate components with their own state.

### Panes, tabs and terminal I/O

- `Pane::spawn` (`src/pane.rs`) creates a PTY and a `Term` behind an `Arc<FairMutex<…>>`, and starts alacritty's
  `EventLoop` thread for the I/O. Dropping a `Pane` sends `Msg::Shutdown`, which hangs up the shell.
- Terminal events reach the UI through the `Listener`. It sends `(PaneId, Event)` on an mpsc channel and calls
  `request_repaint()`. `process_term_events` drains the channel every frame (titles, PTY replies, clipboard, child
  exit closes the pane).
- `STerm.panes: HashMap<PaneId, Pane>` owns the sessions. Each `Tab` holds a `layout::Node` split tree whose leaves
  are `PaneId`s. `app/panes.rs` lays out the tree, animates rects toward their targets, and calls `pane.resize`
  from the resulting rect.
- The working folder and the foreground program come from `/proc/<pid>/cwd` and `/proc/<pid>/stat` (tpgid).
  `Pane::at_empty_prompt` is a heuristic that looks for a prompt character before the cursor. Several features
  depend on these, so the app is Linux-only.

### Input

`input::action_for` maps keys to an `Action`, and `STerm::do_action` carries it out. Keys with no action go through
`input::key_to_bytes` to `send_input`, which writes to every pane in the tab when broadcast is on. While
`modal_open()` is true (palette, settings, setup or viewer), overlays take all keyboard input and nothing reaches the
shell.

- To add an action: add the `Action` variant, its binding in `action_for`, its handling in `do_action`, an entry in
  the palette's command list (`app/palette.rs`), and a row in the README shortcut table.
- Clickable widgets use `app::CLICK`, `CLICK_DRAG` and `DRAG` (and an equivalent `CLICK` in sidebar/usage_ui).
  These `Sense`s are never keyboard-focusable, so Tab, Space and Enter always go to the shell. Don't use the default
  `Sense::click()` for anything that sits in the main window.

### Settings

`STerm.config` is what the UI edits directly, and `STerm.applied` is the last applied copy. `apply_config` compares
them every frame, applies the side effects (theme, fonts, zoom, decorations) and saves atomically to
`~/.config/s-term/config.json` (write to `.json.tmp`, then rename). `Config` is `#[serde(default)]`, so a new field
needs a sensible `Default` value, and old config files still load. The prompt setting reaches bash through flag
files (`~/.config/s-term/prompt-dir`, `prompt-user`), which `scripts/s-term-prompt.sh` checks at every prompt.

### Background threads

Nothing slow runs on the UI thread. Each worker publishes a snapshot (usually `Arc<Mutex<Option<Arc<Snapshot>>>>`)
or sends on a channel, then calls `ctx.request_repaint()`:

- **Sidebar** (`sidebar.rs`, thread `s-term-sidebar`): a git and file-tree snapshot of the focused pane's folder
  about every 1.2 s, plus diff and preview loads on request (`Req`). The UI sends `follow(cwd)` every 500 ms. Every
  git call sets `GIT_OPTIONAL_LOCKS=0` so it never takes `index.lock` while an agent runs git in a pane. Keep that
  setting.
- **Usage** (`usage.rs`, 3 s refresh): reads Claude Code's `~/.claude/projects/**.jsonl` incrementally (appended
  bytes only), Codex's `~/.codex/sessions`, and Antigravity's SQLite databases (read-only, protobuf blobs). It keeps
  its own `~/.local/share/s-term/usage-daily.json`. The data of other tools is read-only: never write to it.
  Antigravity's plan quota isn't on disk: `AgyQuotaPoll` runs `agy -p /usage --output-format json` as a background
  child (every minute while `agy` runs, every 10 minutes otherwise, only while Antigravity is on in Settings). Don't
  read its login from the keyring instead.
  `usage_ui.rs` draws the AI view and the status-bar meter.
- **Assistant**: see below.

### Launchpad (`src/launchpad.rs` + `src/app/launch.rs`)

`Config::launchers` holds the saved launchers. `launchpad.rs` draws the view inside the sidebar and edits that list;
it asks for runs through `UiAction::Launcher(id, Op)`. `app/launch.rs` owns the runs: `launch_tick` types a queued
command line once the shell is at a prompt, and refreshes `launch_status` every 250 ms for the panel, rail and tab
labels. A launcher owns its pane only while its own line runs (`Run::active`). After that, Stop must not send `Ctrl+C`
to whatever the user ran there next. Between `a && b` the shell has the foreground for a moment, so it takes two idle
reads in a row before a run counts as finished.

While a text field in the side panel has focus, `Sidebar::typing()` is true and `handle_keyboard` keeps keys away
from the shell. Any new panel text field must set it (read the focus from `ui.memory`, so focus requested in the same
frame counts).

### History (`src/history.rs` + `src/app/history.rs`)

`close_pane` records the pane (or its whole tab, when it's the last pane there) before `drop_pane` removes it;
`close_tab` records the tab once and then uses `drop_pane`. Anything that closes panes without the user meaning to
(the untouched first pane a restored session replaces) must call `drop_pane`. A pane whose shell has exited can't be
read from `/proc` any more, so `history_tick` reads every pane every 2 s into `HistState::live` and the entry is made
from that. Claude Code's conversation id comes from `~/.claude/sessions/<pid>.json`, keyed by the foreground process
group (`Pane::foreground_pgid`); Codex's from the rollout file it has open. Both are read only. Restoring runs the
agent's resume command, but only types other programs' command lines without Enter. Ids are checked to be uuid-like,
and lines with control characters are dropped, because all of it is typed into a shell.

`~/.local/share/s-term/history.json` is shared by every window: each change re-reads it, edits and writes it back.
Each window also rewrites `sessions/<pid>.json` with its open tabs; `on_exit` turns those into a session entry, and at
startup `adopt_orphans` does the same for windows that died without `on_exit`. Screenshot mode keeps the history in
memory and never touches these files.

### Ask S-term (`src/assistant/` + `src/app/ask.rs`)

- `assistant::start` runs the conversation loop on a worker thread (at most `MAX_STEPS` = 30 tool rounds). The
  system prompt and the tool schemas are in `assistant/mod.rs`.
- The UI thread owns the panes, so the worker can't run tools itself. It sends `Event::Tool(call, reply_sender)` and
  waits. `STerm::ask_tick` (`app/ask.rs`) queues the calls, and `run_tool` either runs one right away or holds it as
  `Pending` until the user approves (unless `config.ai_auto_run` is on). `check_waiting` decides when a command has
  finished: the shell is back in the foreground and the output has stopped changing.
- `provider.rs` speaks one `Turn` format to Anthropic, OpenAI and Gemini over plain HTTPS (`ureq`). An assistant
  turn keeps the provider's raw reply and sends it back unchanged to the same provider, because thinking blocks and
  thought signatures must round-trip. After a provider switch, turns are rebuilt from text and tool calls.
- API keys live in `~/.config/s-term/keys.json` (mode 0600), kept out of `config.json` on purpose. If no key is
  saved, the provider's env var is used (`keys.rs`).
- To add a tool: add its `ToolDef` in `tools()`, a line in `describe()`, and its handling in `ask.rs`
  (`run_tool`/`execute`). Decide whether it needs approval. Reads run right away, while anything that types, runs
  commands or closes panes asks first.

### Rendering and look

- `render.rs` paints the grid cell by cell with a `GlyphCache` of laid-out single glyphs. `ensure_metrics` snaps cell
  sizes to whole physical pixels.
- `theme.rs`: a `Theme` holds both the 16-color terminal palette and the UI chrome colors. A new preset needs an
  entry in `theme::PRESETS` (id and display name) and a match arm in `Theme::preset`. `glass.rs` draws the frosted surfaces and the backdrop.
- The window is frameless and transparent by default. `chrome.rs` draws the rounded corners, the title bar and the
  edge-resize handling. `config.system_titlebar` switches to native decorations.
- The Ask popup can open in its own OS viewport (`show_viewport_immediate`, id `s-term-ask`), so the terminal stays
  usable while it is open.
- Fonts (JetBrains Mono, Inter) are embedded with `include_bytes!`. `assets/icon/app-64.rgba` and `glyph-256.rgba`
  are raw RGBA8 (64×64 and 256×256), also embedded at compile time. If you change the icon, regenerate them.

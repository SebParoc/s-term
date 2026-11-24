# ⚡🦂 S-term

A modern terminal for Linux, written in Rust.

Arrange your work in splits and tabs, see your repo's changes and files without leaving the terminal, start a
project's commands with one click, and reopen anything you closed. If you use AI coding tools, S-term can also show
their context and plan limits, resume their conversations in reopened tabs, and give you an assistant that reads your
panes and runs commands once you say OK. All of that is optional.

- **Splits and tabs**: Terminator-style splits with an animated layout, pane maximize, and typing broadcast to every
  pane in a tab
- **Explorer and Source control**: the focused pane's project tree and git changes, with a diff, file, Markdown and
  image viewer
- **Launchpad**: save the commands that start a project (say `npm install`, then `npm run dev`) and run them in a new
  tab with one click
- **History**: reopen closed tabs, panes and your last session with their folders and splits. Claude Code and Codex
  conversations resume.
- **Command palette**: every action, view, theme, setting and launcher, a few keystrokes away
- **Seven themes**, glass surfaces, its own title bar, and settings that apply as you change them
- **Ask S-term** (optional): an assistant in the title bar that reads your panes, opens new ones and runs commands with
  your approval. Bring your own Anthropic, OpenAI or Gemini API key.
- **AI usage** (optional): live context, plan limits, tokens per model and an activity heatmap for Claude Code, the
  Codex CLI and the Antigravity CLI

S-term uses [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal) for terminal emulation, scrollback
and the PTY, and [`egui`/`eframe`](https://github.com/emilk/egui) for the window and everything drawn in it.

## Contents

- [Install](#install)
- [The window](#the-window)
- [Panes and tabs](#panes-and-tabs)
- [Ask S-term](#ask-s-term)
- [Side panel](#side-panel): [Explorer](#explorer), [Source control](#source-control), [Launchpad](#launchpad),
  [History](#history), [AI usage](#ai-usage)
- [Command palette](#command-palette)
- [Settings](#settings)
- [Shell prompt](#shell-prompt)
- [Keyboard and mouse](#keyboard-and-mouse)
- [Files and privacy](#files-and-privacy)
- [Command line](#command-line)
- [Development](#development)

## Install

S-term runs on 64-bit Linux, since it reads `/proc` to follow each shell's folder and the program running in it.

### Download a release (no Rust needed)

Each [release](https://github.com/SebParoc/s-term/releases) has a tarball with the app and an install script. It
runs on any distribution with glibc 2.35 or newer: Ubuntu 22.04, Debian 12, Fedora 36 and later. With the GitHub
CLI (it uses your GitHub login, which a private repo needs):

```sh
gh release download --repo SebParoc/s-term --pattern '*-linux.tar.gz'
tar xzf s-term-*-linux.tar.gz
./s-term-*-linux/install.sh
```

Or download the `.tar.gz` from the releases page, extract it, and run the `install.sh` inside. The script copies
`s-term` to `~/.local/bin` and adds the icon, the app menu entry and the [prompt script](#shell-prompt), so you can
delete the downloaded files afterwards. To update, do the same with the newest release.

### Build from source

You need a recent stable Rust toolchain (from [rustup.rs](https://rustup.rs) or your distribution), a C compiler for
the bundled SQLite (`sudo dnf install gcc` or `sudo apt install build-essential`), and git.

```sh
git clone https://github.com/SebParoc/s-term.git
cd s-term
./scripts/install.sh
```

The script builds the release binary, links `~/.local/bin/s-term` to it, and adds the icon, the app menu entry and
the prompt script. The app runs straight from `target/release` in your clone, so keep the folder where it is. Every
`cargo build --release` updates the installed app (reopen S-term to see the changes), and
`git pull && ./scripts/install.sh` updates everything. To try S-term without installing it, run
`cargo run --release`.

### After installing

S-term is in your app menu, ready to pin to your dock. It uses `git` for Source control and `xdg-open` to open files
in other apps. `install.sh --uninstall` removes it again. Your settings and History stay in `~/.config/s-term` and
`~/.local/share/s-term` until you delete those folders.

The first time S-term starts, a short setup asks which AI coding tools you use and which provider the assistant
should talk to. Both are optional, and you can change them later in Settings.

## The window

- **Title bar:** your tabs, **+** for a new tab, the **Ask S-term** box and the close button (minimize and maximize
  are optional). Drag a tab to reorder it and middle-click it to close it. A tab's icon shows when it has splits, when
  it's broadcasting, when a launcher runs in it (the tab then takes the launcher's name), and which AI CLI runs in its
  pane: Claude Code, Codex or Antigravity. Drag empty space to move
  the window, and double-click it to maximize. Settings → Appearance → *System title bar* uses your desktop's title
  bar instead.
- **Activity rail** (left edge): Explorer, Source control (with the number of changed files), Launchpad (a green dot
  while a launcher runs), History, and AI usage (a dot that goes green, yellow or red with Claude Code's context).
  Click an icon to open its view in the side panel, and click it again to close it. Settings is the gear at the bottom.
- **Status bar:** branch and changes, the folder, the AI usage meter, the broadcast indicator, the number of panes in
  the tab, the grid size, and the theme and font size. Click an item to open the matching view.

## Panes and tabs

- `Ctrl+Shift+E` splits side by side and `Ctrl+Shift+O` splits top and bottom. The new pane opens in the focused
  pane's folder. Drag a divider or press `Ctrl+Shift+Arrows` to resize, and press `Alt+Arrows` to move focus.
- `Ctrl+Shift+X` maximizes the focused pane, and pressing it again restores the layout.
- `Alt+A` sends what you type to every pane in the tab, and `Alt+O` stops.
- `Ctrl+Shift+T` opens a tab in the focused pane's folder (or your home folder: Settings → Terminal → *New tabs open
  in*). `Ctrl+Shift+N` opens a new window in that folder.
- `Ctrl+Shift+W` closes the pane, and `Ctrl+Shift+R` brings it back (see [History](#history)). Closing the last pane
  closes the window.
- **Drag and drop:** drop files on a pane to paste their quoted paths. Claude Code turns a pasted image path into an
  attachment. Drop a single folder on a pane that's at an empty prompt to `cd` into it. Drop folders on the **+** to
  open a tab in each one.
- **Copy and paste:** `Ctrl+Shift+C` and `Ctrl+Shift+V`. Plain `Ctrl+C` interrupts as usual, and plain `Ctrl+V` goes
  to the program, so Claude Code can paste an image from the clipboard. Turn on Settings → Terminal → *Copy on
  select* to copy as soon as you select.
- `Shift+Enter` types a newline without submitting, in Claude Code and tools like it.

Each pane can have a title bar showing what runs in it, with the icon of Claude Code, Codex or Antigravity when one of
them does: always, only when the tab has splits, or never. Panes
without focus can be dimmed. Both are in Settings → Appearance.

## Ask S-term

Click **Ask S-term** in the title bar and type a question or a task, like "why did my build fail?", "what's using
port 8080?" or "open a pane on the right and start the dev server". The same box searches commands, so it doubles as
the [command palette](#command-palette). The conversation opens in its own small window that stays on top and can be
moved anywhere, so you can keep using the terminal while the assistant works. Drag its header to move it, and drag an
edge or corner to resize it. It keeps that size (saved across restarts), and a long conversation scrolls inside it.

The assistant sees your tabs and panes and the last lines of the focused pane, and it has tools to:

- list the tabs and panes, and read any pane's text, scrollback included
- open a pane to the right or below, or a new tab, and focus a pane
- run a command in a pane that's idle at a shell prompt, and read the result
- type into a running program, for example to answer a prompt
- close a pane

Reading and opening panes happen right away. Running a command, typing and closing a pane wait for your OK: the
command shows up in the conversation, where you can edit it and press **Run** (or `Enter`), or **Skip** it. Settings
→ Assistant → *Run commands without asking* lets the assistant act on its own. It won't type into a pane that's busy
with another program, such as an editor, a server or an agent, unless you ask it to. It opens a new pane instead.

**Setup:** pick Anthropic, OpenAI or Gemini in Settings → Assistant and paste an API key. Keys are saved to
`~/.config/s-term/keys.json`, readable only by you. Without a saved key, S-term uses `ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, or `GEMINI_API_KEY` / `GOOGLE_API_KEY`. The model picker lists the models your key can use.
Anthropic defaults to Claude Opus 5.5, and the others use the newest model unless you pick one.

**Privacy:** nothing is sent until you ask something. Each message goes to the provider you picked, with the list of
panes and the focused pane's last 40 lines. The assistant reads other panes only through its tools, and every tool
it uses is listed in the conversation.

## Side panel

`Ctrl+Shift+B` shows or hides the side panel, and the rail icons switch between its views. The panel follows the
focused pane's folder and refreshes about once a second on a background thread. It never takes git's index lock, so
it won't get in the way of git commands an agent is running.

### Explorer

The project tree of the focused pane's folder. Changed files are colored, and git-ignored files are dimmed (or hidden,
see Settings → Explorer).

- **Click a file** to preview it. Markdown opens formatted, with a toggle for the source. PNG, JPEG, GIF, WebP, BMP
  and ICO files open as images. The viewer can copy the selection or the whole file, open the file in its default
  app, and copy the path or type it into the terminal. `Esc` closes it.
- **Middle-click a file** to type its path into the terminal. **Right-click** it to do the same, copy the path, or
  open it in the default app. Right-click a folder to add it as its own section, or to make a launcher for it.
- **Drag a file** onto a pane to paste its path.
- **More folders:** add one with the **+** in the Explorer header, by right-clicking a folder, or in Settings →
  Explorer. Each folder gets a section under the focused pane's project, which you can fold or remove. Drag the line
  between two sections to resize them, and double-click it to reset.

### Source control

The branch (with how far it's ahead of or behind its upstream) and git status, with the most recently touched files
first, their +/− line counts and how long ago they changed. Files changed in the last 90 seconds get an accent dot.
Click a file to see its diff.

Recent commits are listed underneath. A commit is tagged when Claude or Gemini wrote it or co-wrote it, going by the
author or a `Co-authored-by` line. Right-click a commit to type or copy its hash, or copy its author.

### Launchpad

The Launchpad starts projects in one click. A **launcher** is a folder plus the commands you'd type there every time,
for example `npm install` and then `npm run dev`. Press its play button and S-term opens a new tab in the folder,
waits for the prompt, and types `npm install && npm run dev`, so each command runs only if the one before it
succeeded. The output stays in a normal terminal tab, where you can watch it, type into it or press `Ctrl+C`.

- **Make one** with the **+** in the Launchpad header, from the command palette, or by right-clicking a folder in the
  Explorer or an entry in History. The editor fills in the name and folder from the focused pane's folder, and you
  type the commands, one per line. Blank lines and `#` comments are skipped. Leave the folder empty to run in whatever
  folder the focused pane is in. `Ctrl+Enter` saves and `Esc` cancels.
- **Run in** a new tab (the default), a split to the right, or the focused pane. *This pane* types into the focused
  pane only when it's idle at a prompt, and otherwise opens a new tab.
- **While it runs,** its play button turns into a stop button (`Ctrl+C`), the row shows the running program and its
  tab, and the tab is named after the launcher. Click the row to go to that tab. Pressing play again later reruns it
  in the same tab if that tab is back at its prompt.
- **Right-click** a launcher to restart it, go to its tab, close its pane, edit, duplicate, reorder or delete it, or
  copy the exact command line it types.
- Every launcher is also in the command palette as **Run: *name***.

Launchers are saved with your settings, in `~/.config/s-term/config.json`.

### History

History (`Ctrl+Shift+H`) lists the tabs and panes you closed, newest first, grouped by day. Click one to reopen it in
a new tab, or press `Ctrl+Shift+R` to reopen the newest. A closed shell can't come back to life, so reopening starts
new shells from what was saved:

- **The layout and folders:** a tab with splits comes back with the same splits, each pane in its old folder.
- **Claude Code and Codex conversations resume:** S-term types `claude --resume <id>` (keeping `--model`,
  `--permission-mode` and `--dangerously-skip-permissions`) or `codex resume <id>`. It finds the id while the agent
  runs, from Claude Code's `~/.claude/sessions/<pid>.json` and from the rollout file Codex has open.
- **Other programs are typed but not run:** a pane closed while running `npm run dev` comes back with `npm run dev`
  at the prompt, so you decide whether to press `Enter`.
- **The last session:** when a window closes with tabs open, they become one *Session* entry. Next time S-term starts,
  a popup at the bottom offers to reopen them until you start typing. Turn on Settings → Terminal → *Reopen last
  session at startup* to reopen them without asking. Windows that crashed or were killed at logout are caught too,
  because each window keeps its open tabs in `~/.local/share/s-term/sessions/<pid>.json`.

Right-click an entry to reopen a single pane as a split, copy its folder, make a launcher for it, or delete it. The
trash button in the header clears everything. A lone idle shell in your home folder isn't kept, and the list holds
the newest 60 entries.

History is saved in `~/.local/share/s-term/history.json`, readable only by you, since it holds folder names and
command lines. Turn it off in Settings → Terminal → *Remember closed tabs*.

### AI usage

Usage tracking is optional. During setup, S-term asks which AI coding tools you use (Claude Code, Codex, Antigravity)
and turns on the ones it finds. You can change them in Settings → AI tools. The AI view has four sections:

- **Now:** each tool's live session, with a context ring that turns yellow at 50% and red at 75%
- **Plan limits:** Claude Code's 5-hour and weekly limits, and Antigravity's for each of its model groups, with reset
  countdowns
- **Usage:** tokens per model across the tools you turned on, for the last 5 hours, today or the last 7 days, plus a
  24-hour output chart. Hover a model for its breakdown.
- **Activity:** a calendar heatmap of daily tokens (output tokens or all tokens, see Settings → AI tools). Pick a
  model to see only that model, and hover a day for its details.

The meter in the status bar follows the tool running in the focused pane (or, when that pane runs none, whichever is
running: Claude Code, then Codex, then Antigravity). It shows the tool's context and its 5-hour and weekly limits: Claude
Code's plan limits, Antigravity's for the model group of the conversation in use, and for Codex the tokens of the last
5 hours and 7 days. Click it to open the AI view.

Token counts come from Claude Code's transcripts in `~/.claude/projects`, Codex's session logs in `~/.codex/sessions`
and Antigravity's conversation databases in `~/.gemini/antigravity-cli/conversations`, all read-only. S-term also
keeps its own daily totals in `~/.local/share/s-term/usage-daily.json`, so your history outlives transcript cleanup.
S-term itself never sends usage anywhere. The only request it causes is Antigravity's own quota check (see below).

#### Claude Code plan limits

Claude Code shares its plan limits and its exact context-window size only with its status-line command. To let
S-term read them, add this to `~/.claude/settings.json`:

```json
"statusLine": { "type": "command", "command": "~/.local/bin/s-term statusline" }
```

`s-term statusline` saves what Claude Code sends to `~/.cache/s-term/claude-status/` and prints a short line such as
`Opus 5.5 · ctx 42% · 5h 38% · week 12%`. Settings → AI tools shows whether the hook is connected.

#### Antigravity plan limits

Antigravity keeps its quota on Google's servers, not on disk, so S-term runs the CLI's own report,
`agy -p /usage --output-format json`, in the background: about once a minute while `agy` is running, and every 10
minutes otherwise. It's the same check `/usage` does inside `agy`. It uses no quota, and `--log-file /dev/null` keeps
it from leaving a log file each time. S-term never reads Antigravity's login. Turning Antigravity off in
Settings → AI tools stops it.

## Command palette

`Ctrl+Shift+P`, or a click on the box in the title bar, opens a fuzzy search over:

- every action, with its shortcut
- the side panel's views, and actions such as *Explorer: add folder…* and *Launchpad: new launcher…*
- themes (*Theme: Ember*) and quick toggles (animations, cursor glide, status bar, title bars, the prompt…)
- every setting, which opens Settings at the right section
- your launchers (*Run: name*)

Type a question instead and the top row asks S-term, as long as you've added an API key.

## Settings

Open Settings with `Ctrl+,` or the gear at the bottom of the rail. Changes apply immediately and are saved to
`~/.config/s-term/config.json`.

| Section | What's in it |
|---|---|
| Appearance | Theme (Neon, Midnight, Ember, Aurora, Graphite, Atelier, Full Moon), font family, size and line height, what zoom scales (terminal text or the whole interface), animations, cursor glide and blink, pane title bars, dimming inactive panes, the status bar, the system title bar, minimize and maximize buttons |
| Terminal | Shell command, scrollback, where new tabs open, what goes before the `❯`, cursor style, scroll speed, copy on select, remembering closed tabs, reopening the last session at startup |
| Explorer | Extra folders, hidden files, git-ignored files, opening the panel at startup |
| Assistant | Provider and API key, model, running commands without asking |
| AI tools | Which tools to track, what the activity heatmap measures, the Claude Code status-line hook |
| Keybindings | Every shortcut |
| About | Version, a tour of the features (click a tile to open it), where your data is kept, running setup again, resetting settings (launchers, API keys and History are kept) |

## Shell prompt

`install.sh` copies the prompt script (`scripts/s-term-prompt.sh`) to `~/.bashrc.d/`. If you don't have that
folder, it saves the script in `~/.local/share/s-term/` and shows the line to add to `~/.bashrc`. Inside S-term the
script turns the bash prompt into a plain `❯` that goes red after a failed command, since the pane's title bar
already shows the folder. Other terminals keep your usual prompt.

Settings → Terminal → *Before the ❯* adds the folder (`~/s-term ❯`), or user@host and the folder
(`user@host:~/s-term ❯`). Every open pane picks up the change at its next prompt.

## Keyboard and mouse

### Panes and tabs

| Keys | Action |
|---|---|
| `Ctrl+Shift+E` | Split side by side |
| `Ctrl+Shift+O` | Split top and bottom |
| `Ctrl+Shift+W` | Close pane |
| `Ctrl+Shift+R` | Reopen the last closed tab, pane or session |
| `Alt+←/→/↑/↓` | Focus the neighboring pane |
| `Ctrl+Shift+←/→/↑/↓` | Resize pane |
| `Ctrl+Shift+X` | Maximize or restore pane |
| `Alt+A` / `Alt+O` | Start / stop broadcasting typing to every pane in the tab |
| `Ctrl+Shift+T` | New tab |
| `Ctrl+Tab` or `Ctrl+PageDown` | Next tab |
| `Ctrl+Shift+Tab` or `Ctrl+PageUp` | Previous tab |
| `Ctrl+Shift+N` | New window, in the focused pane's folder |

### Everything else

| Keys | Action |
|---|---|
| `Ctrl+Shift+P` | Command palette (type a question to ask S-term) |
| `Ctrl+,` | Settings |
| `Ctrl+Shift+B` | Show or hide the side panel |
| `Ctrl+Shift+H` | Show or hide History |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste |
| `Shift+PageUp/PageDown/Home/End` | Scroll back |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0`, or `Ctrl` + wheel | Zoom in / out / reset (terminal text, or the whole interface: Settings → Appearance) |
| `Shift+Enter` or `Alt+Enter` | Newline without submitting (sends `ESC CR`, which Claude Code understands) |
| `Esc` | Close the diff or file viewer, the palette or Settings |
| `Ctrl+Shift+Q` | Quit |

**Mouse:** click a pane to focus it, drag to select, double-click to select a word and triple-click to select a line.
Scroll for scrollback. Full-screen programs that ask for mouse input get it, so hold `Shift` to select text in them
anyway. Drag a divider to resize panes, and middle-click a tab to close it.

## Files and privacy

| Path | What's in it |
|---|---|
| `~/.config/s-term/config.json` | Settings and launchers |
| `~/.config/s-term/keys.json` | API keys, readable only by you |
| `~/.config/s-term/prompt-dir`, `prompt-user` | Flags the [prompt script](#shell-prompt) checks |
| `~/.local/share/s-term/history.json` | [History](#history), readable only by you |
| `~/.local/share/s-term/sessions/<pid>.json` | Each open window's tabs, so a crashed window's session isn't lost |
| `~/.local/share/s-term/usage-daily.json` | Daily token totals |
| `~/.local/share/s-term/s-term-prompt.sh` | The prompt script, when you don't have `~/.bashrc.d` |
| `~/.cache/s-term/claude-status/` | What Claude Code's status line last reported |

S-term follows `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_CACHE_HOME` when they're set. Settings → About opens
these folders.

S-term reads Claude Code's, Codex's and Antigravity's own files (transcripts, session logs, conversation databases,
`~/.claude/sessions`) without ever changing them. The only network traffic is the assistant's, which goes only to
the provider you picked, and Antigravity's own quota check when that tool is turned on ([details](#antigravity-plan-limits)).

## Command line

| Command | What it does |
|---|---|
| `s-term` | Opens a window. Its first tab starts in the current folder. |
| `s-term statusline` | The Claude Code status-line hook (see [Claude Code plan limits](#claude-code-plan-limits)) |
| `s-term usage` | Prints the token totals the AI view shows, per tool and model |
| `S_TERM_FONT="Family Name" s-term` | Forces a terminal font |

## Development

```sh
cargo build --release              # the installed app runs target/release/s-term
cargo test                         # unit tests; `cargo test <name>` runs one
cargo test -- --ignored live_      # tests that call the real AI provider APIs
```

The code isn't formatted with rustfmt, so don't run `cargo fmt`. Match the style around your change. CLAUDE.md has
notes on the architecture.

**Screenshot mode** opens a real window, sets up the views you name, saves a PNG and exits, without touching your
settings or History:

```sh
S_TERM_SCREENSHOT=out.png S_TERM_DEMO=split,ai ./target/release/s-term
```

The steps are listed in `src/app/devshot.rs` (`settings`, `section=about`, `palette`, `ask`, `launchdemo`,
`historydemo`, `theme=ember` and more).

**Releases:** bump `version` in `Cargo.toml`, then push a matching tag. GitHub Actions
(`.github/workflows/release.yml`) builds the tarball on Ubuntu 22.04, so it runs on older distributions too, and
publishes it as a release:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

`./scripts/package.sh` builds the same tarball locally in `dist/`, but that one only runs on distributions with a
glibc as new as yours.

### Code map

| Path | What it does |
|---|---|
| `src/main.rs` | Entry point, and the `statusline` and `usage` subcommands |
| `src/app.rs` | Tabs, the pane tree, input dispatch and the frame loop |
| `src/app/chrome.rs` | Title bar, activity rail, status bar and window resizing |
| `src/app/panes.rs` | Pane layout animation, mouse handling and painting |
| `src/app/palette.rs` | Command palette and the Ask S-term popup |
| `src/app/ask.rs` | The conversation view, and the assistant's tools (reading panes, running commands, approvals) |
| `src/app/launch.rs` | Runs launchers: opens their pane, types the commands at the prompt, tracks what they're doing |
| `src/app/history.rs` | Records tabs as they close and the window's session, the startup popup, reopening entries |
| `src/app/settings.rs` | Settings page, including About |
| `src/app/setup.rs` | First-run setup screen |
| `src/app/devshot.rs` | Screenshot mode |
| `src/assistant/` | Ask S-term: the tool-calling loop, Anthropic / OpenAI / Gemini requests, API keys |
| `src/pane.rs` | One shell session (PTY and emulator state), and reading its folder and program from `/proc` |
| `src/layout.rs` | The split tree |
| `src/render.rs` | Grid painting |
| `src/input.rs` | Keybindings, and keys to escape sequences |
| `src/sidebar.rs` | Side panel: git and files on a background thread, Explorer, Source control, and the diff/file viewer |
| `src/launchpad.rs` | Launchpad view: the launcher list and editor, and the command lines they type |
| `src/history.rs` | History: entries and their files, reading what a pane runs (agent conversation ids), the History view |
| `src/usage.rs` | Claude Code, Codex and Antigravity usage readers |
| `src/usage_ui.rs` | AI view, its icons and the status-bar meter |
| `src/config.rs` | Settings file |
| `src/theme.rs` | Color themes |
| `src/fonts.rs` | Bundled fonts and the installed-font picker |
| `src/glass.rs` | Glass surfaces, the backdrop, the selection pill and the search orb |
| `src/icons.rs` | Vector line icons |
| `scripts/install.sh` | Installs from a clone (builds it) or from a release tarball, and `--uninstall` |
| `scripts/package.sh` | Builds the release tarball |
| `scripts/s-term-prompt.sh` | The bash prompt for S-term |

## Roadmap

- Search in scrollback
- Clickable URLs
- Italic font faces
- Scrollback in reopened tabs (History brings back folders, splits and agent conversations, not the output)
- A drop-down (quake-style) mode

## Credits

S-term ships **JetBrains Mono** for the terminal and **Inter** for the interface, both under the SIL Open Font
License. Their licenses are in `assets/fonts/`. You can pick any installed monospace font in Settings. The scorpion
"S" app icon is in `assets/icon/`.

## License

MIT

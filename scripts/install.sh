#!/usr/bin/env bash
# Install S-term and add it to the app menu. Works from a clone of the repo (builds it, needs Rust)
# and from an extracted release tarball (copies the prebuilt binary, no Rust needed).
#
#   install.sh               install, or update: run it again after `git pull` or a new download
#   install.sh --uninstall   remove the app menu entry, icon, s-term command and prompt script
#
# From a clone, ~/.local/bin/s-term links to target/release/s-term, so keep the checkout where it
# is, and every later `cargo build --release` updates the installed app.
set -euo pipefail

# The repo root (this script lives in scripts/) or the release folder (it sits at the top).
ROOT="$(cd "$(dirname "$0")" && pwd)"
[ "$(basename "$ROOT")" = scripts ] && ROOT="$(dirname "$ROOT")"
cd "$ROOT"

BIN_DIR="$HOME/.local/bin"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APPS_DIR="$DATA_HOME/applications"
ICONS_DIR="$DATA_HOME/icons/hicolor"
SIZES="16 24 32 48 64 128 256 512"
# The prompt script goes in ~/.bashrc.d when it exists (Fedora sources it), otherwise next to S-term's data.
PROMPT_RC="$HOME/.bashrc.d/s-term-prompt.sh"
PROMPT_DATA="$DATA_HOME/s-term/s-term-prompt.sh"

say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

refresh_caches() {
  touch "$ICONS_DIR" 2>/dev/null || true
  gtk-update-icon-cache -q "$ICONS_DIR" 2>/dev/null || true
  update-desktop-database -q "$APPS_DIR" 2>/dev/null || true
}

uninstall() {
  say "Removing S-term"
  rm -f "$BIN_DIR/s-term" "$APPS_DIR/s-term.desktop" "$ICONS_DIR/scalable/apps/s-term.svg" "$PROMPT_RC" "$PROMPT_DATA"
  for n in $SIZES; do
    rm -f "$ICONS_DIR/${n}x${n}/apps/s-term.png"
  done
  refresh_caches
  say "Done. Your settings and History are still in ~/.config/s-term and ~/.local/share/s-term."
  note "Delete those folders to remove them too."
  if grep -qs "s-term-prompt.sh" "$HOME/.bashrc"; then
    note "Also remove the line that sources s-term-prompt.sh from ~/.bashrc."
  fi
  if grep -qs "s-term statusline" "$HOME/.claude/settings.json"; then
    note "Also remove the \"statusLine\" entry that runs s-term from ~/.claude/settings.json."
  fi
}

case "${1:-}" in
  "") ;;
  --uninstall) uninstall; exit 0 ;;
  -h | --help) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) die "unknown option '$1' (try --help)" ;;
esac

[ "$(uname -s)" = Linux ] || die "S-term runs on Linux only."
mkdir -p "$BIN_DIR" "$APPS_DIR"

if [ -f Cargo.toml ]; then
  # ------------------------------------------------------------ from a clone: build it
  # rustup puts cargo in ~/.cargo/bin, which a fresh shell may not have on its PATH yet.
  if ! command -v cargo >/dev/null && [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
  command -v cargo >/dev/null || die "cargo not found. Install Rust from https://rustup.rs (or your distribution's packages) and run this again, or download a prebuilt release instead."
  command -v cc >/dev/null || command -v gcc >/dev/null || die "no C compiler found (needed to build the bundled SQLite). Install one: 'sudo dnf install gcc' or 'sudo apt install build-essential'."

  say "Building S-term (release). The first build takes a few minutes."
  cargo build --release
  say "Linking $BIN_DIR/s-term -> $ROOT/target/release/s-term"
  rm -f "$BIN_DIR/s-term"
  ln -s "$ROOT/target/release/s-term" "$BIN_DIR/s-term"
  UPDATE="git pull && ./scripts/install.sh"
elif [ -f s-term ]; then
  # ------------------------------------------------------------ from a release: copy the binary
  say "Copying S-term to $BIN_DIR/s-term"
  rm -f "$BIN_DIR/s-term"
  install -m755 s-term "$BIN_DIR/s-term"
  UPDATE="download the newest release and run its install.sh"
else
  die "nothing to install here: run this from a clone of the S-term repo or from an extracted release."
fi
command -v git >/dev/null || note "git isn't installed: S-term works, but the Source control panel stays empty."

say "Adding the icon and the app menu entry"
# The scorpion "S" in every size the desktop asks for.
for n in $SIZES; do
  install -Dm644 "assets/icon/s-term-$n.png" "$ICONS_DIR/${n}x${n}/apps/s-term.png"
done
# GNOME prefers the scalable icon over the PNGs, so it must be the new one too.
install -Dm644 assets/icon/s-term.svg "$ICONS_DIR/scalable/apps/s-term.svg"
cat > "$APPS_DIR/s-term.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=S-term
Comment=A modern terminal, written in Rust
Exec=$BIN_DIR/s-term
Icon=s-term
Terminal=false
Categories=System;TerminalEmulator;Utility;
StartupWMClass=s-term
DESKTOP
refresh_caches

# Short prompt for bash inside S-term (reads Settings → Terminal → "Before the ❯").
if [ -d "$HOME/.bashrc.d" ]; then
  install -m644 scripts/s-term-prompt.sh "$PROMPT_RC"
  say "Added the S-term prompt to ~/.bashrc.d/"
else
  install -Dm644 scripts/s-term-prompt.sh "$PROMPT_DATA"
  if ! grep -qs "s-term-prompt.sh" "$HOME/.bashrc"; then
    say "For the S-term prompt, add this line to ~/.bashrc:"
    note "source \"$PROMPT_DATA\""
  fi
fi

say "Installed. S-term is in your app menu."
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) note "$BIN_DIR isn't on your PATH, so 's-term' won't run from a shell (the app menu works)."
     note "Add it in ~/.bashrc:  export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac
note "To update later: $UPDATE"

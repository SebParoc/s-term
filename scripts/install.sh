#!/usr/bin/env bash
# Build S-term and link it into the app menu. The menu entry points at the
# release build, so every later `cargo build --release` updates it automatically.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release
mkdir -p ~/.local/bin ~/.local/share/applications
ln -sf "$PWD/target/release/s-term" ~/.local/bin/s-term

# App icon (scorpion "S") in every size the desktop asks for.
for n in 16 24 32 48 64 128 256 512; do
  mkdir -p ~/.local/share/icons/hicolor/${n}x${n}/apps
  cp assets/icon/s-term-$n.png ~/.local/share/icons/hicolor/${n}x${n}/apps/s-term.png
done
# GNOME prefers the scalable icon over the PNGs, so it must be the new one too.
mkdir -p ~/.local/share/icons/hicolor/scalable/apps
cp assets/icon/s-term.svg ~/.local/share/icons/hicolor/scalable/apps/s-term.svg
touch ~/.local/share/icons/hicolor
gtk-update-icon-cache -q ~/.local/share/icons/hicolor 2>/dev/null || true
update-desktop-database -q ~/.local/share/applications 2>/dev/null || true

cat > ~/.local/share/applications/s-term.desktop <<DESKTOP
[Desktop Entry]
Type=Application
Name=S-term
Comment=A terminal for vibe coding, with Terminator-style splits
Exec=$HOME/.local/bin/s-term
Icon=s-term
Terminal=false
Categories=System;TerminalEmulator;Utility;
StartupWMClass=s-term
DESKTOP

# Short prompt for bash inside S-term (reads Settings → Terminal → "Before the ❯").
if [ -d ~/.bashrc.d ]; then
  cp scripts/s-term-prompt.sh ~/.bashrc.d/s-term-prompt.sh
else
  echo "To get the S-term prompt, source $PWD/scripts/s-term-prompt.sh from ~/.bashrc"
fi

echo "Installed: ~/.local/bin/s-term -> $PWD/target/release/s-term"

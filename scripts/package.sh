#!/usr/bin/env bash
# Build a release tarball for people without Rust: dist/s-term-<version>-<arch>-linux.tar.gz.
# It holds the binary, the icons, the prompt script and install.sh, which copies them into place.
#
# The binary needs the glibc it was built against or newer, so official releases are built on an
# older distro (see .github/workflows/release.yml). A local build only runs on distros as new as yours.
set -euo pipefail
cd "$(dirname "$0")/.."

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)"
name="s-term-$version-$(uname -m)-linux"
out="dist/$name"

cargo build --release --locked

rm -rf "$out" "$out.tar.gz" "$out.tar.gz.sha256"
mkdir -p "$out/scripts" "$out/assets/icon" "$out/licenses"
install -m755 target/release/s-term "$out/s-term"
strip "$out/s-term" 2>/dev/null || true
install -m755 scripts/install.sh "$out/install.sh"
install -m644 scripts/s-term-prompt.sh "$out/scripts/"
install -m644 assets/icon/s-term-*.png assets/icon/s-term.svg "$out/assets/icon/"
install -m644 README.md "$out/"
install -m644 assets/fonts/Inter-LICENSE.txt assets/fonts/JetBrainsMono-OFL.txt "$out/licenses/"

tar -C dist -czf "$out.tar.gz" "$name"
(cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
rm -rf "$out"

glibc="$(objdump -T target/release/s-term 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n1 || true)"
echo "Built $out.tar.gz ($(du -h "$out.tar.gz" | cut -f1)), needs ${glibc:-an unknown glibc} or newer"

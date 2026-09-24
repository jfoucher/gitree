#!/usr/bin/env bash
# Builds Gitree in release mode and installs it.
#   scripts/install.sh              -> installs into ~/.local
#   sudo PREFIX=/usr/local scripts/install.sh
#   scripts/install.sh --uninstall
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PREFIX=${PREFIX:-$HOME/.local}
APP=io.github.gitree.Gitree

files=(
  "$PREFIX/bin/gitree"
  "$PREFIX/share/applications/$APP.desktop"
  "$PREFIX/share/metainfo/$APP.metainfo.xml"
  "$PREFIX/share/icons/hicolor/scalable/apps/$APP.svg"
)

if [[ "${1:-}" == "--uninstall" ]]; then
  rm -f "${files[@]}"
  echo "Removed Gitree from $PREFIX"
  exit 0
fi

# Build as the normal user; under sudo reuse an existing release build.
if [[ $EUID -ne 0 ]]; then
  (cd "$ROOT" && cargo build --release)
elif [[ ! -x "$ROOT/target/release/gitree" ]]; then
  echo "Run 'cargo build --release' as your user first." >&2
  exit 1
fi

install -Dm755 "$ROOT/target/release/gitree" "${files[0]}"
install -Dm644 "$ROOT/data/$APP.desktop" "${files[1]}"
install -Dm644 "$ROOT/data/$APP.metainfo.xml" "${files[2]}"
install -Dm644 "$ROOT/data/icons/scalable/apps/$APP.svg" "${files[3]}"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" || true
command -v gtk4-update-icon-cache >/dev/null && gtk4-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" || true
echo "Installed Gitree to $PREFIX (binary: ${files[0]})"
case ":$PATH:" in *":$PREFIX/bin:"*) ;; *) echo "Note: $PREFIX/bin is not on your PATH";; esac

#!/usr/bin/env bash
# Builds Gitree as a Flatpak and installs it for the current user.
#   scripts/flatpak.sh            -> build + install (run: flatpak run io.github.gitree.Gitree)
#   scripts/flatpak.sh --bundle   -> also write gitree.flatpak, installable on any distro with
#                                    `flatpak install --user gitree.flatpak`
# Needs only flatpak; the builder, GNOME SDK and Rust extension are installed from Flathub
# (per user) on first run.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=io.github.gitree.Gitree
MANIFEST="$ROOT/flatpak/$APP.yml"
SOURCES="$ROOT/flatpak/cargo-sources.json"
GEN_URL=https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
BUILD="$ROOT/target/flatpak"

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user -y --noninteractive flathub org.flatpak.Builder \
  org.gnome.Sdk//50 org.gnome.Platform//50 org.freedesktop.Sdk.Extension.rust-stable//25.08

# The build runs offline, so every crate in Cargo.lock is listed as a source.
if [[ ! -f "$SOURCES" || "$ROOT/Cargo.lock" -nt "$SOURCES" ]]; then
  echo "Regenerating $SOURCES"
  mkdir -p "$BUILD"
  curl -sSfL -o "$BUILD/flatpak-cargo-generator.py" "$GEN_URL"
  if command -v uv >/dev/null; then
    uv run -q "$BUILD/flatpak-cargo-generator.py" "$ROOT/Cargo.lock" -o "$SOURCES"
  else
    echo "Needs uv (or: pip install aiohttp PyYAML tomlkit, then python3 $BUILD/flatpak-cargo-generator.py)" >&2
    exit 1
  fi
fi

flatpak run --filesystem="$ROOT" org.flatpak.Builder --user --install --force-clean \
  --state-dir="$BUILD/state" --repo="$BUILD/repo" "$BUILD/app" "$MANIFEST"

if [[ "${1:-}" == "--bundle" ]]; then
  flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    "$BUILD/repo" "$ROOT/gitree.flatpak" "$APP"
  echo "Wrote $ROOT/gitree.flatpak"
fi
echo "Run with: flatpak run $APP"

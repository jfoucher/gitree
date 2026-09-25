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

# The gtk-rs crates find GTK through pkg-config, which needs the distro's
# development packages (headers + .pc files), not just the runtime libraries.
check_build_deps() {
  local missing=()
  command -v pkg-config >/dev/null || missing+=(pkg-config)
  command -v cc >/dev/null || missing+=(cc)
  if command -v pkg-config >/dev/null; then
    local mod
    for mod in 'gtk4 >= 4.18' 'libadwaita-1 >= 1.7' 'gtksourceview-5'; do
      pkg-config --exists "$mod" || missing+=("$mod")
    done
  fi
  [[ ${#missing[@]} -eq 0 ]] && return 0

  local ids="" cmd
  [[ -r /etc/os-release ]] && ids=$(. /etc/os-release; echo "${ID:-} ${ID_LIKE:-}")
  case " $ids " in
    *" fedora "*|*" rhel "*)
      cmd="sudo dnf install gcc pkgconf-pkg-config gtk4-devel libadwaita-devel gtksourceview5-devel" ;;
    *" debian "*|*" ubuntu "*)
      cmd="sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libgtksourceview-5-dev" ;;
    *" arch "*)
      cmd="sudo pacman -S --needed base-devel gtk4 libadwaita gtksourceview5" ;;
    *" suse "*|*" opensuse "*)
      cmd="sudo zypper install gcc pkgconf gtk4-devel libadwaita-devel gtksourceview5-devel" ;;
    *)
      cmd="install the development packages for GTK 4 (>= 4.18), libadwaita (>= 1.7) and GtkSourceView 5" ;;
  esac
  echo "Missing build dependencies: ${missing[*]}" >&2
  echo "Install them with:" >&2
  echo "  $cmd" >&2
  exit 1
}

# Build as the normal user; under sudo reuse an existing release build.
if [[ $EUID -ne 0 ]]; then
  check_build_deps
  (cd "$ROOT" && cargo build --release)
elif [[ ! -x "$ROOT/target/release/gitree" ]]; then
  echo "Run 'cargo build --release' as your user first." >&2
  exit 1
fi

install -Dm755 "$ROOT/target/release/gitree" "${files[0]}"
# Absolute Exec path: GLib hides desktop entries whose binary isn't on the
# session PATH, and ~/.local/bin is often only added by the shell profile.
install -dm755 "$(dirname "${files[1]}")"
sed "s|^Exec=gitree|Exec=${files[0]}|" "$ROOT/data/$APP.desktop" > "${files[1]}"
chmod 644 "${files[1]}"
install -Dm644 "$ROOT/data/$APP.metainfo.xml" "${files[2]}"
install -Dm644 "$ROOT/data/icons/scalable/apps/$APP.svg" "${files[3]}"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" || true
# Only refresh an existing theme cache; without one, lookups scan the directory.
if [[ -f "$PREFIX/share/icons/hicolor/icon-theme.cache" ]] && command -v gtk4-update-icon-cache >/dev/null; then
  gtk4-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
fi
echo "Installed Gitree to $PREFIX (binary: ${files[0]})"
case ":$PATH:" in *":$PREFIX/bin:"*) ;; *) echo "Note: $PREFIX/bin is not on your PATH";; esac

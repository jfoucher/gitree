#!/usr/bin/env bash
# Renders Gitree in a private headless GNOME Shell and saves a PNG.
# Usage: scripts/screenshot.sh <repo-dir> <out.png> [actions] [delay-ms]
#   actions: "name=arg;name2" — repo actions to trigger before the capture
# Uses a private config dir and D-Bus session so your desktop is untouched.
set -euo pipefail
REPO=${1:?repo (use - for none)}; OUT=${2:?out.png}; ACTIONS=${3:-}; DELAY=${4:-2000}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
CFG=${GITREE_TEST_CONFIG:-$(mktemp -d)}
LOG=$(mktemp)
rm -f "$OUT"
timeout 60 dbus-run-session -- bash -c "
  gnome-shell --headless --wayland --no-x11 --virtual-monitor 1280x820 --wayland-display gitree-shot >>'$LOG' 2>&1 &
  SP=\$!
  for i in \$(seq 50); do [ -S \"\$XDG_RUNTIME_DIR/gitree-shot\" ] && break; sleep 0.2; done
  sleep 1
  XDG_CONFIG_HOME='$CFG' WAYLAND_DISPLAY=gitree-shot GDK_BACKEND=wayland \
    $( [ -n "${GITREE_AUTO_ACCEPT:-}" ] && echo GITREE_AUTO_ACCEPT=1 ) GITREE_SCREENSHOT='$OUT' GITREE_DEBUG_ACTIONS='$ACTIONS' GITREE_SCREENSHOT_DELAY='$DELAY' \
    timeout 40 '$ROOT/target/debug/gitree' $( [ "$REPO" = - ] || echo "'$REPO'" ) 2>&1 | grep -v -E '^$|Gtk-WARNING|^\(gitree' || true
  kill \$SP
" >>"$LOG" 2>&1 || true
if [ -f "$OUT" ]; then echo "saved $OUT"; else echo "screenshot failed, see $LOG"; exit 1; fi

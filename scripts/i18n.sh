#!/usr/bin/env bash
# Translation catalog maintenance (needs GNU gettext >= 0.24 for Rust).
#   scripts/i18n.sh pot             -> regenerate po/gitree.pot from po/POTFILES.in
#   scripts/i18n.sh update          -> pot, then merge it into every po/<lang>.po
#   scripts/i18n.sh check           -> fail if a source file with strings is missing from POTFILES.in
#   scripts/i18n.sh build <prefix>  -> compile catalogs into <prefix>/share/locale and write the
#                                      translated desktop entry and metainfo under <prefix>/share
# `build target` gives a development build (target/debug/gitree) its catalogs.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PO="$ROOT/po"
DOMAIN=gitree
APP=io.github.gitree.Gitree

langs() { grep -v '^#' "$PO/LINGUAS" | tr -s ' \n' '\n' | sed '/^$/d'; }

pot() {
  (cd "$ROOT" && xgettext \
    --files-from=po/POTFILES.in \
    --directory="$ROOT" \
    --output=po/$DOMAIN.pot \
    --from-code=UTF-8 \
    --add-comments=Translators \
    --keyword=gettext_f \
    --keyword=ngettext_f:1,2 \
    --keyword=N_ \
    --package-name=Gitree \
    --package-version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)" \
    --msgid-bugs-address=https://github.com/jfoucher/gitree/issues \
    --sort-by-file)
  echo "Wrote po/$DOMAIN.pot ($(grep -c '^msgid ' "$PO/$DOMAIN.pot") messages)"
}

check() {
  local missing
  missing=$(cd "$ROOT" && grep -rlE '\b(n?gettext(_f)?|pgettext|N_)\(' src --include='*.rs' \
    | grep -v '^src/i18n.rs$' | sort | comm -23 - <(grep -v '^#' po/POTFILES.in | sort))
  if [[ -n "$missing" ]]; then
    echo "Missing from po/POTFILES.in:" >&2
    echo "$missing" >&2
    exit 1
  fi
}

case "${1:-}" in
  pot)
    check
    pot
    ;;
  update)
    check
    pot
    for l in $(langs); do
      msgmerge --quiet --update --backup=none "$PO/$l.po" "$PO/$DOMAIN.pot"
      echo "Updated po/$l.po"
    done
    ;;
  check)
    check
    ;;
  build)
    prefix=${2:?usage: i18n.sh build <prefix>}
    for l in $(langs); do
      install -dm755 "$prefix/share/locale/$l/LC_MESSAGES"
      msgfmt --check -o "$prefix/share/locale/$l/LC_MESSAGES/$DOMAIN.mo" "$PO/$l.po"
    done
    install -dm755 "$prefix/share/applications" "$prefix/share/metainfo"
    msgfmt --desktop -d "$PO" --template="$ROOT/data/$APP.desktop" -o "$prefix/share/applications/$APP.desktop"
    # Needs the metainfo ITS rules shipped with appstream; fall back to the untranslated file.
    msgfmt --xml -d "$PO" --template="$ROOT/data/$APP.metainfo.xml" -o "$prefix/share/metainfo/$APP.metainfo.xml" \
      || { echo "i18n.sh: installing untranslated metainfo" >&2
           install -m644 "$ROOT/data/$APP.metainfo.xml" "$prefix/share/metainfo/$APP.metainfo.xml"; }
    ;;
  *)
    sed -n '2,8p' "$0" >&2
    exit 1
    ;;
esac

//! Translations through gettext. Catalogs live in `po/`, maintained with
//! `scripts/i18n.sh`.
//!
//! Messages with runtime values use named `{placeholders}` so translators can
//! reorder them: `gettext_f("Delete {name}?", &[("name", &branch)])`.

use std::path::PathBuf;

pub use gettextrs::{gettext, ngettext, pgettext};

pub const DOMAIN: &str = "gitree";

/// Sets up the locale and the message catalog. Call before building any UI.
pub fn init() {
    // SAFETY: called first thing in main, before any other thread exists.
    unsafe {
        gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    }
    let _ = gettextrs::bindtextdomain(DOMAIN, locale_dir());
    let _ = gettextrs::bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = gettextrs::textdomain(DOMAIN);
}

/// `GITREE_LOCALEDIR`, else `<prefix>/share/locale` next to the executable's
/// `bin/` (covers ~/.local, /usr, /app in Flatpak and `target/share/locale`
/// for development builds).
fn locale_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("GITREE_LOCALEDIR") {
        return dir.into();
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("share/locale")))
        .unwrap_or_else(|| "/usr/share/locale".into())
}

fn substitute(mut s: String, args: &[(&str, &str)]) -> String {
    for (key, value) in args {
        s = s.replace(&format!("{{{key}}}"), value);
    }
    s
}

/// Translates `msgid` and fills in its `{name}` placeholders.
pub fn gettext_f(msgid: &str, args: &[(&str, &str)]) -> String {
    substitute(gettext(msgid), args)
}

/// Plural form of [`gettext_f`]; `{n}` is replaced by `n`.
pub fn ngettext_f(msgid: &str, msgid_plural: &str, n: u32, args: &[(&str, &str)]) -> String {
    let n_str = n.to_string();
    let mut all = vec![("n", n_str.as_str())];
    all.extend_from_slice(args);
    substitute(ngettext(msgid, msgid_plural, n), &all)
}

/// Marks a string for extraction without translating it, for tables that are
/// translated where they are displayed.
#[allow(non_snake_case)]
pub const fn N_(msgid: &str) -> &str {
    msgid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_substituted() {
        assert_eq!(
            substitute("Merge {a} into {b}".into(), &[("a", "x"), ("b", "y")]),
            "Merge x into y"
        );
        assert_eq!(ngettext_f("{n} file", "{n} files", 3, &[]), "3 files");
    }
}

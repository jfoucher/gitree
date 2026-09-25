# Gitree

A native GNOME Git client inspired by a source control tool on macOS, written in Rust
with GTK 4 and libadwaita. It drives the real `git` command line, so hooks,
credential helpers, signing, LFS and your git config all behave exactly as in a
terminal.

## Features

| Feature | Gitree |
|---|---|
| Repository browser: bookmarks, groups, search, New → Clone / Add / Create / Scan | “Repositories” tab (Ctrl+T) with the same actions, branch + change counts per repo |
| Tabbed repository windows | One tab per repository; open tabs are restored on start |
| Toolbar: Commit, Pull, Push, Fetch, Branch, Merge, Stash, Discard, Tag, Git-flow, Remote, Terminal, Finder, Settings | Same toolbar, with ahead/behind badges on Pull/Push |
| Sidebar: File Status / History / Search, Branches (folders for `feature/…`), Tags, Remotes, Stashes, Submodules, Subtrees | Same tree, with context menus on every item |
| File Status: staged/unstaged lists, checkboxes, flat/tree view, filters, search | Same (filters: pending, conflicted, untracked, modified, ignored) |
| Stage / unstage / discard **hunks and individual lines** | Hunk buttons; select lines (or click/shift-click the line-number gutter) to stage/unstage/discard lines |
| Commit box: amend, push immediately, sign-off, bypass hooks, message history | Same, plus GPG signing; Ctrl+Enter commits |
| History: commit graph, ref badges, branch filter, date/ancestor order, jump to commit | Same; select two commits to diff them |
| Commit context menu: checkout, merge, rebase, tag, branch, archive, patch, reset (soft/mixed/hard), reverse, cherry-pick, rebase children interactively, copy SHA | All implemented |
| Search commits by message, file changes, author | Same, plus search by SHA |
| Interactive rebase: reorder (drag), squash, reword, edit, delete | Same dialog |
| Merge / rebase / cherry-pick in progress banner with Continue / Abort | Same (plus Skip) |
| Conflicts: resolve using mine/theirs, external merge tool, mark (un)resolved | Same |
| Blame (“Annotate”), Log selected file | Blame window (click a line to jump to the commit) and file log window |
| Git-flow: init, start/finish feature, release, hotfix | Native implementation, compatible with git-flow AVH config |
| Git LFS, submodules, subtrees | LFS dialog (track/untrack, fetch/pull/push/prune), submodule add/update/sync/open/remove, subtree add/pull/push |
| Repository settings: remotes, identity, .gitignore, config | Same (edit .gitignore, info/exclude, .git/config, .gitattributes in-app) |
| Preferences: identity, diff/merge tool, fetch interval, custom actions, … | Same; custom actions get `$REPO`, `$SHA`, `$FILE` |
| Credentials prompt | Built-in askpass dialog for HTTPS passwords and SSH passphrases; credential helper selectable in Preferences |
| Auto refresh | File watcher (ignores .gitignored build output) + refresh on focus + periodic background fetch |

Not included: hosting-service accounts (GitHub/Bitbucket/GitLab login, pull requests) and Mercurial.

## Keyboard shortcuts

| Action | Shortcut |
|---|---|
| New tab / close tab | Ctrl+T / Ctrl+W |
| Open / clone repository | Ctrl+O / Ctrl+Shift+N |
| File Status / History / Search | Ctrl+1 / Ctrl+2 / Ctrl+3 |
| Commit, Pull, Push, Fetch | Ctrl+Shift+C, L, P, F |
| Branch, Merge, Stash, Tag, Discard | Ctrl+Shift+B, M, S, T, R |
| Refresh | Ctrl+R / F5 |
| Terminal / Show in Files | Ctrl+Alt+T / Ctrl+Alt+O |
| Preferences | Ctrl+, |

## Building

Building needs gettext and the GTK 4 (>= 4.18), libadwaita (>= 1.7) and
GtkSourceView 5 development packages. Having the libraries installed isn't enough; pkg-config
needs their `-dev`/`-devel` files. `scripts/install.sh` checks for them and prints
the command for your distro.

```sh
# Debian / Ubuntu
sudo apt install build-essential pkg-config gettext libgtk-4-dev libadwaita-1-dev libgtksourceview-5-dev git
# Fedora
sudo dnf install gcc pkgconf-pkg-config gettext gtk4-devel libadwaita-devel gtksourceview5-devel git
# Arch
sudo pacman -S --needed base-devel gettext gtk4 libadwaita gtksourceview5 git
# openSUSE
sudo zypper install gcc pkgconf gettext-tools gtk4-devel libadwaita-devel gtksourceview5-devel git
# optional everywhere: git-lfs meld

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust toolchain
```

Run from the source tree:

```sh
cargo run --release -- /path/to/repo      # path is optional
```

Install for the current user (binary in `~/.local/bin`, desktop entry and icon):

```sh
scripts/install.sh               # or: sudo PREFIX=/usr/local scripts/install.sh
scripts/install.sh --uninstall
```

### Flatpak

The Flatpak bundles GTK, libadwaita and GtkSourceView, so it runs on any distro
that has Flatpak. You don't need the development packages or Rust installed:

```sh
scripts/flatpak.sh            # builds and installs for the current user
scripts/flatpak.sh --bundle   # also writes gitree.flatpak to copy to other machines
flatpak run io.github.gitree.Gitree
```

Inside the sandbox, Gitree runs `git`, your terminal and custom actions on the
host through `flatpak-spawn --host`. Your own git, hooks, credential helpers,
ssh/gpg agents and LFS work as they do natively, so the host still needs `git`.
The Flatpak keeps its settings in `~/.var/app/io.github.gitree.Gitree/`,
separate from a native install.

Settings and bookmarks are stored in `~/.config/gitree/settings.json`.

## Development

```sh
cargo test          # parsers, graph layout, patch generation + integration tests on temp repos
cargo clippy
scripts/make-test-repo.sh /tmp/demo [--conflict]   # repo with branches, tags, stash, submodule, remote
scripts/screenshot.sh /tmp/demo/work shot.png "show-status;debug-select-unstaged=0"
```

`screenshot.sh` runs the app in a private headless GNOME Shell and saves a PNG after
triggering the given repository actions (any `repo.*` action name, `name=argument`,
separated by `;`). With `GITREE_AUTO_ACCEPT=1`, option dialogs are accepted with their
defaults, which makes it possible to exercise Push, Pull, Stash, … end to end.

### Translations

User-visible strings go through gettext (`src/i18n.rs`); the catalogs live in `po/`
with the text domain `gitree`. Wrap new strings in `gettext("…")`, and use
`gettext_f("Delete {branch}?", &[("branch", &name)])` or `ngettext_f` rather than
`format!`, so translators see whole sentences with named placeholders.

```sh
scripts/i18n.sh pot       # regenerate po/gitree.pot (needs gettext >= 0.24 for Rust)
scripts/i18n.sh update    # regenerate the template and merge it into every po/<lang>.po
```

To add a language, run `msginit -i po/gitree.pot -o po/<lang>.po -l <lang>` and add
its code to `po/LINGUAS`. `scripts/install.sh` and the Flatpak compile the catalogs
and merge the translations into the desktop entry and metainfo. To try one from the
source tree, run `scripts/i18n.sh build target`, then `LANGUAGE=<lang> cargo run`.
New source files with strings go in `po/POTFILES.in`; `scripts/i18n.sh check`
lists any that are missing.

### Layout

- `src/git/` — git CLI wrapper and parsers (status, refs, log, graph lanes, diff and
  partial patches, blame, rebase todo, git-flow). No GTK code.
- `src/ui/` — windows, views and dialogs. `dialogs.rs` holds the `repo.*` actions
  shared by the toolbar, menus and keyboard shortcuts.
- `src/askpass.rs` — credential prompt used when git runs Gitree as `GIT_ASKPASS`.
- `src/watch.rs` — file watcher driving auto refresh.
- `data/` — stylesheet, icons, desktop entry and metainfo.
- `po/` — translation template (`gitree.pot`), translations and `POTFILES.in`.

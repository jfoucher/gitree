# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Gitree is a native GNOME Git client modelled on Sourcetree for macOS: Rust (edition 2024), GTK 4 (`v4_18`), libadwaita (`v1_7`), GtkSourceView 5. It drives the real `git` CLI rather than a library, so hooks, credential helpers, signing and LFS behave as in a terminal.

## Commands

```sh
cargo run -- /path/to/repo          # path optional; --release for a fast build
cargo test                          # unit tests + integration tests on temp repos (needs `git` on PATH)
cargo test <name_substring>         # single test, e.g. cargo test stage_unstage_and_discard_single_lines
cargo test git::integration_tests   # one module
cargo clippy
scripts/make-test-repo.sh /tmp/demo [--conflict]   # repo with branches, tags, stash, submodule, bare "origin"
scripts/screenshot.sh /tmp/demo/work shot.png "show-status;debug-select-unstaged=0" [delay-ms]
scripts/install.sh [--uninstall]    # installs to ~/.local (PREFIX overrides)
```

System deps (Debian names): `libgtk-4-dev libadwaita-1-dev libgtksourceview-5-dev pkg-config`.

`screenshot.sh` runs `target/debug/gitree` (so `cargo build` first) inside a private headless GNOME Shell with a throwaway `XDG_CONFIG_HOME`, triggers the given `repo.*` actions (`name=arg;name2`), saves a PNG of the newest visible window, and quits. This is the way to check UI changes visually. Set `GITREE_AUTO_ACCEPT=1` to auto-accept confirmation dialogs and `Form`s with their defaults so Push, Pull, Stash and similar flows run end to end.

Other dev env vars: `GITREE_DEBUG_REFRESH` (logs refreshes and watcher triggers), `GITREE_KEEP_OPEN` (don't quit after a screenshot), `GITREE_TEST_CONFIG` (config dir for `screenshot.sh`).

## Architecture

**Two layers with a hard boundary:**
- `src/git/` is GTK-free. It shells out to git and parses machine-readable output (status, refs, log, graph lane layout, diffs and partial patches, blame, rebase todo, git-flow). Unit tests live next to the parsers. `integration_tests.rs` runs real git in `tempfile` repos.
- `src/ui/` holds the GTK code. Views are plain Rust structs held in `Rc<...>` (no GObject subclassing), with `weak()` upgrades used in signal closures.

**All git invocations go through `git::Git` / `base_command` (`src/git/runner.rs`).** It forces `LC_ALL=C.UTF-8`, `color.ui=false`, `core.quotepath=false`, `GIT_EDITOR=true` and `GIT_OPTIONAL_LOCKS=0`, and strips `GIT_DIR`/`GIT_WORK_TREE`, so output is stable to parse. Never spawn `git` directly. `Prompt::Interactive` sets `GIT_ASKPASS`/`SSH_ASKPASS` to Gitree's own executable. `Prompt::Never` (the default for `run`) disables prompting, which background fetches rely on.

**Askpass re-entry:** when git launches the binary with `GITREE_ASKPASS` set, `main.rs` short-circuits into `askpass::run()` (a standalone credential dialog) before the GTK app starts.

**Threading:** blocking git calls must not run on the main loop. Use `ui::spawn(async { ... })` for main-loop futures and `ui::bg(|| ...).await` (a `gio::spawn_blocking` wrapper) for the git work.

**State flow per repository tab (`ui/repo_view.rs`):**
- `RepoView::refresh()` loads an immutable `Snapshot` (status, refs, stashes, remotes, submodules, subtrees, in-progress op state, git-flow config) on a worker thread, then `apply_snapshot` pushes it to the sidebar, file status view, banners and ahead/behind badges.
- History reloads only when the refs signature or dirty state changes (`last_log_key`). Call `invalidate_log()` to force a reload.
- Refreshes are coalesced through the `refreshing`/`busy`/`refresh_pending` flags. Triggers are the file watcher (`src/watch.rs`, which ignores gitignored paths), window focus and a periodic fetch.
- Mutating operations should use `rv.run_ops(title, cmds, OpOptions)`, which shows the streaming, cancellable progress sheet (`ui/progress.rs`) and refreshes afterwards.

**Actions:** toolbar buttons, context menus, keyboard shortcuts and `GITREE_DEBUG_ACTIONS` all go through `repo.<name>` actions that take a string argument. To add one, register its name in `ACTIONS` and add a match arm in `handle()`, both in `src/ui/dialogs.rs`. `dispatch()` is the entry point. Option dialogs are built with the `ui/form.rs` `Form` helper.

**Other pieces:**
- `config.rs`: settings, bookmarks and open tabs, stored as JSON in `$XDG_CONFIG_HOME/gitree/settings.json`.
- `ui/window.rs`: the tabbed main window. It restores open tabs and has a "Repositories" browser tab (`ui/browser.rs`).
- `data/`: the stylesheet, symbolic icons, desktop file and metainfo. They are compiled into a GResource by `build.rs` (manifest: `data/resources.gresource.xml`), so new icon or CSS files must be listed there. The resource base path is `/io/github/gitree/Gitree`, and the app ID is `io.github.gitree.Gitree`.

`playground/` (untracked) holds unrelated side experiments, such as `git-garden`, a Python script. It is not part of the app.

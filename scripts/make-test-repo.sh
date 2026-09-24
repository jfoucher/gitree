#!/usr/bin/env bash
# Builds a throwaway repository exercising most Gitree features:
# branches, merges, tags, stashes, a bare "origin" remote, a submodule,
# staged/unstaged/untracked changes and an optional merge conflict.
#
# Usage: scripts/make-test-repo.sh <dir> [--conflict]
set -euo pipefail

DIR=${1:?usage: make-test-repo.sh <dir> [--conflict]}
CONFLICT=${2:-}
rm -rf "$DIR"
mkdir -p "$DIR"
cd "$DIR"

export GIT_AUTHOR_NAME="Ada Lovelace" GIT_AUTHOR_EMAIL="ada@example.com"
export GIT_COMMITTER_NAME="Ada Lovelace" GIT_COMMITTER_EMAIL="ada@example.com"
g() { git -c init.defaultBranch=main -c protocol.file.allow=always "$@"; }

g init -q --bare origin.git
g init -q lib
(cd lib && echo "pub fn lib() {}" > lib.rs && g add . && g commit -qm "Library" )

g init -q work
cd work
g config user.name "Ada Lovelace"
g config user.email "ada@example.com"

cat > main.rs <<'EOF'
fn main() {
    println!("hello");
}
EOF
printf 'target/\n*.log\n' > .gitignore
g add . && g commit -qm "Initial commit"
g remote add origin ../origin.git

for i in 1 2 3; do
  echo "// change $i" >> main.rs
  g commit -qam "Tweak main ($i)"
done
g tag -a v1.0 -m "Version 1.0"

g checkout -qb feature/login
cat > login.rs <<'EOF'
pub fn login(user: &str) -> bool {
    !user.is_empty()
}
EOF
g add login.rs && g commit -qm "Add login module"
echo "// validate" >> login.rs && g commit -qam "Validate users"

g checkout -q main
echo "// hotfix" >> main.rs && g commit -qam "Hotfix on main"
g merge -q --no-ff feature/login -m "Merge branch 'feature/login'"
g tag v1.1

g checkout -qb develop
mkdir -p docs && echo "# Docs" > docs/README.md && g add docs && g commit -qm "Add docs"
g checkout -q main

g push -q origin main develop feature/login --tags 2>/dev/null
g branch -q --set-upstream-to=origin/main main
echo "// local only" >> main.rs && g commit -qam "Local commit not pushed"

g -c protocol.file.allow=always submodule -q add ../lib vendor/lib 2>/dev/null
g commit -qm "Add lib submodule"

# Stash
echo "wip" > wip.txt && g add wip.txt && g stash push -q -m "Work in progress"

if [[ "$CONFLICT" == "--conflict" ]]; then
  g checkout -qb conflicting HEAD~2
  sed -i 's/hello/bonjour/' main.rs && g commit -qam "French greeting"
  g checkout -q main
  sed -i 's/hello/hola/' main.rs && g commit -qam "Spanish greeting"
  g merge conflicting -m "merge" >/dev/null 2>&1 || true
else
  # Working copy changes: staged, unstaged, untracked, deleted.
  sed -i 's/hello/hello, world/' main.rs
  printf 'fn extra() {}\n' >> main.rs
  g add main.rs
  echo "// unstaged tweak" >> login.rs
  echo "new file" > notes.txt
  mkdir -p src/deep && echo "x" > src/deep/new.rs
  g rm -q --cached .gitignore && git checkout -q -- .gitignore 2>/dev/null || true
  echo "build output" > debug.log
fi
echo "Test repo ready at $DIR/work"

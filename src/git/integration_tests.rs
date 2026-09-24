//! Tests running real git commands against throwaway repositories.

use super::diff::{self, ApplyTarget, DiffOptions, Selection};
use super::flow::{FinishOptions, FlowConfig, FlowKind};
use super::rebase::{self, RebaseAction, RebaseItem};
use super::*;
use std::fs;
use std::path::Path;

fn repo() -> (tempfile::TempDir, Git) {
    let dir = tempfile::tempdir().unwrap();
    let git = Git::new(dir.path());
    git.run(&["init", "-q", "-b", "main"]).unwrap();
    git.run(&["config", "user.name", "Test"]).unwrap();
    git.run(&["config", "user.email", "t@example.com"]).unwrap();
    git.run(&["config", "commit.gpgsign", "false"]).unwrap();
    (dir, git)
}

fn write(p: &Path, name: &str, content: &str) {
    if let Some(d) = p.join(name).parent() {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(p.join(name), content).unwrap();
}

fn commit_all(git: &Git, msg: &str) {
    git.run(&["add", "-A"]).unwrap();
    git.run(&["commit", "-q", "-m", msg]).unwrap();
}

const OPTS: DiffOptions = DiffOptions {
    context: 3,
    ignore_whitespace: false,
};

fn line_index(f: &diff::FileDiff, hunk: usize, text: &str, kind: diff::LineKind) -> usize {
    f.hunks[hunk]
        .lines
        .iter()
        .position(|l| l.text == text && l.kind == kind)
        .unwrap_or_else(|| panic!("line {text:?} not found"))
}

#[test]
fn stage_unstage_and_discard_single_lines() {
    let (dir, git) = repo();
    let p = dir.path();
    write(p, "f.txt", "a\nb\nc\nd\ne\n");
    commit_all(&git, "init");
    write(p, "f.txt", "a\nB\nc\nd\ne\nf\n");

    // Stage only the "+B" line (not the deletion of "b", nor "+f").
    let f = &diff::unstaged(&git, "f.txt", OPTS).unwrap()[0];
    let idx = line_index(f, 0, "B", diff::LineKind::Add);
    let sel: Selection = vec![(0, Some([idx].into_iter().collect()))];
    diff::apply_selection(&git, f, &sel, ApplyTarget::Stage, false, OPTS).unwrap();
    assert_eq!(git.run(&["show", ":f.txt"]).unwrap(), "a\nb\nB\nc\nd\ne\n");

    // Unstage it again.
    let f = &diff::staged(&git, "f.txt", OPTS).unwrap()[0];
    let idx = line_index(f, 0, "B", diff::LineKind::Add);
    let sel: Selection = vec![(0, Some([idx].into_iter().collect()))];
    diff::apply_selection(&git, f, &sel, ApplyTarget::Unstage, false, OPTS).unwrap();
    assert_eq!(git.run(&["show", ":f.txt"]).unwrap(), "a\nb\nc\nd\ne\n");

    // Discard only the added "f" line from the work tree.
    let f = &diff::unstaged(&git, "f.txt", OPTS).unwrap()[0];
    let idx = line_index(f, 0, "f", diff::LineKind::Add);
    let sel: Selection = vec![(0, Some([idx].into_iter().collect()))];
    diff::apply_selection(&git, f, &sel, ApplyTarget::Discard, false, OPTS).unwrap();
    assert_eq!(fs::read_to_string(p.join("f.txt")).unwrap(), "a\nB\nc\nd\ne\n");
}

#[test]
fn stage_whole_hunk_among_several() {
    let (dir, git) = repo();
    let p = dir.path();
    let orig: String = (1..=30).map(|i| format!("line {i}\n")).collect();
    write(p, "f.txt", &orig);
    commit_all(&git, "init");
    let changed = orig.replace("line 2\n", "line two\n").replace("line 25\n", "line twenty-five\n");
    write(p, "f.txt", &changed);
    let f = &diff::unstaged(&git, "f.txt", OPTS).unwrap()[0];
    assert_eq!(f.hunks.len(), 2);
    // Stage only the second hunk.
    diff::apply_selection(&git, f, &vec![(1, None)], ApplyTarget::Stage, false, OPTS).unwrap();
    let staged = git.run(&["show", ":f.txt"]).unwrap();
    assert!(staged.contains("line twenty-five"));
    assert!(staged.contains("line 2\n"));
}

#[test]
fn partially_stage_untracked_file() {
    let (dir, git) = repo();
    let p = dir.path();
    write(p, "base.txt", "x\n");
    commit_all(&git, "init");
    write(p, "new.txt", "one\ntwo\nthree\n");
    let f = &diff::untracked(&git, "new.txt", OPTS).unwrap()[0];
    let idx = line_index(f, 0, "two", diff::LineKind::Add);
    let sel: Selection = vec![(0, Some([idx].into_iter().collect()))];
    diff::apply_selection(&git, f, &sel, ApplyTarget::Stage, true, OPTS).unwrap();
    assert_eq!(git.run(&["show", ":new.txt"]).unwrap(), "two\n");
}

#[test]
fn status_refs_and_log_on_real_repo() {
    let (dir, git) = repo();
    let p = dir.path();
    write(p, "a.txt", "1\n");
    commit_all(&git, "first");
    git.run(&["checkout", "-q", "-b", "topic"]).unwrap();
    write(p, "b.txt", "2\n");
    commit_all(&git, "second");
    git.run(&["checkout", "-q", "main"]).unwrap();
    write(p, "c.txt", "3\n");
    commit_all(&git, "third");
    git.run(&["merge", "-q", "--no-ff", "--no-edit", "topic"]).unwrap();
    git.run(&["tag", "-a", "v1", "-m", "v1"]).unwrap();
    write(p, "a.txt", "changed\n");
    write(p, "u.txt", "untracked\n");

    let st = status::status(&git, false).unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert_eq!(st.entries.len(), 2);

    let r = refs::refs(&git).unwrap();
    assert_eq!(r.head_branch.as_deref(), Some("main"));
    assert_eq!(r.locals().count(), 2);
    let tag = r.tags().next().unwrap();
    assert_eq!(Some(&tag.oid), r.head_oid.as_ref(), "annotated tag is peeled");

    let q = log::LogQuery {
        has_head: true,
        order: log::LogOrder::Topo,
        ..Default::default()
    };
    let commits: Vec<_> = log::LogReader::spawn(&git, &q).unwrap().collect();
    assert_eq!(commits.len(), 4);
    assert_eq!(commits[0].parents.len(), 2);
    let mut g = graph::GraphBuilder::new();
    let rows: Vec<_> = commits.iter().map(|c| g.push(&c.oid, &c.parents)).collect();
    assert!(rows[0].is_merge);
    assert_eq!(rows.iter().map(|r| r.width).max(), Some(2));
    assert_eq!(rows.last().unwrap().col, 0);

    let files = diff::commit_files(&git, &commits[0].oid).unwrap();
    assert_eq!(files.len(), 1, "merge diff is against the first parent");
    assert_eq!(files[0].path, "b.txt");
}

#[test]
fn interactive_rebase_squash_reword_drop() {
    let (dir, git) = repo();
    let p = dir.path();
    write(p, "f.txt", "0\n");
    commit_all(&git, "base");
    for i in 1..=4 {
        write(p, &format!("f{i}.txt"), &format!("{i}\n"));
        commit_all(&git, &format!("c{i}"));
    }
    let base = git.run(&["rev-parse", "HEAD~4"]).unwrap().trim().to_string();
    let commits = log::range_oldest_first(&git, Some(&base)).unwrap();
    assert_eq!(commits.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["c1", "c2", "c3", "c4"]);
    let item = |i: usize, action, msg: Option<&str>| RebaseItem {
        oid: commits[i].oid.clone(),
        subject: commits[i].subject.clone(),
        action,
        new_message: msg.map(String::from),
    };
    let items = vec![
        item(0, RebaseAction::Pick, Some("c1 and c2")),
        item(1, RebaseAction::Squash, None),
        item(2, RebaseAction::Drop, None),
        item(3, RebaseAction::Reword, Some("fourth")),
    ];
    let todo_dir = dir.path().join(".git").join("gitree-rebase");
    let todo = rebase::write_todo(&items, &todo_dir).unwrap();
    let handle = std::sync::Arc::new(std::sync::Mutex::new(None));
    git.run_streaming(
        &["rebase", "-i", &base],
        Prompt::Never,
        &[("GIT_SEQUENCE_EDITOR".into(), rebase::sequence_editor(&todo))],
        handle,
        |_| {},
    )
    .unwrap();
    let subjects = git.run(&["log", "--format=%s", "-3"]).unwrap();
    assert_eq!(subjects, "fourth\nc1 and c2\nbase\n");
    assert!(!p.join("f3.txt").exists(), "dropped commit's file is gone");
    assert!(p.join("f2.txt").exists());
}

#[test]
fn git_flow_feature_and_release() {
    let (dir, git) = repo();
    let p = dir.path();
    write(p, "a.txt", "1\n");
    commit_all(&git, "init");
    let cfg = FlowConfig::default();
    let run_all = |cmds: Vec<Vec<String>>| {
        for c in cmds {
            git.run(&c).unwrap_or_else(|e| panic!("{e}"));
        }
    };
    run_all(cfg.init_commands(true, false, true));
    assert_eq!(flow::config(&git).unwrap().develop, "develop");

    run_all(cfg.start_commands(FlowKind::Feature, "login", None));
    write(p, "login.txt", "x\n");
    commit_all(&git, "login");
    run_all(cfg.finish_commands(
        FlowKind::Feature,
        "login",
        &FinishOptions {
            delete_branch: true,
            ..Default::default()
        },
    ));
    let r = refs::refs(&git).unwrap();
    assert_eq!(r.head_branch.as_deref(), Some("develop"));
    assert!(r.find_local("feature/login").is_none());

    run_all(cfg.start_commands(FlowKind::Release, "1.0", None));
    run_all(cfg.finish_commands(
        FlowKind::Release,
        "1.0",
        &FinishOptions {
            delete_branch: true,
            ..Default::default()
        },
    ));
    let r = refs::refs(&git).unwrap();
    assert!(r.tags().any(|t| t.name == "1.0"));
    // main contains the feature.
    assert!(git.run(&["show", "main:login.txt"]).is_ok());
}

#[test]
fn op_state_detects_conflicted_merge() {
    let (dir, git) = repo();
    let p = dir.path();
    write(p, "f.txt", "base\n");
    commit_all(&git, "base");
    git.run(&["checkout", "-q", "-b", "other"]).unwrap();
    write(p, "f.txt", "theirs\n");
    commit_all(&git, "theirs");
    git.run(&["checkout", "-q", "main"]).unwrap();
    write(p, "f.txt", "mine\n");
    commit_all(&git, "mine");
    assert!(git.run(&["merge", "other"]).is_err());
    let st = status::status(&git, false).unwrap();
    assert!(st.has_conflicts());
    assert_eq!(state::op_state(&git.git_dir()), state::OpState::Merge);
    git.run(&["checkout", "--theirs", "--", "f.txt"]).unwrap();
    git.run(&["add", "f.txt"]).unwrap();
    git.run(&["commit", "--no-edit"]).unwrap();
    assert_eq!(state::op_state(&git.git_dir()), state::OpState::None);
    assert_eq!(fs::read_to_string(p.join("f.txt")).unwrap(), "theirs\n");
}

#[test]
fn blame_and_stash() {
    let (dir, git) = repo();
    let p = dir.path();
    write(p, "f.txt", "one\ntwo\n");
    commit_all(&git, "first");
    write(p, "f.txt", "one\nTWO\n");
    commit_all(&git, "second");
    let b = blame::blame(&git, "f.txt", None).unwrap();
    assert_eq!(b.lines.len(), 2);
    assert_ne!(b.lines[0].oid, b.lines[1].oid);
    assert_eq!(b.commits[&b.lines[1].oid].summary, "second");

    write(p, "f.txt", "dirty\n");
    git.run(&["stash", "push", "-m", "wip"]).unwrap();
    let s = refs::stashes(&git).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].name, "stash@{0}");
    assert!(s[0].message.contains("wip"));
}

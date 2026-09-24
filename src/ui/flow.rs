//! Git-flow dialogs: initialise, start and finish feature / release / hotfix.

use super::form::{combo_value, text_of, Form};
use super::progress::OpOptions;
use super::repo_view::RepoView;
use crate::git::flow::{FinishOptions, FlowConfig, FlowKind};
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

pub async fn show(rv: &Rc<RepoView>) {
    let snap = rv.snapshot();
    let Some(cfg) = snap.flow.clone() else {
        init(rv).await;
        return;
    };
    let current = snap.current_branch().unwrap_or("").to_string();
    let form = Form::new("Git-flow", "Close");
    form.description("What would you like to do?");
    let choice = Rc::new(Cell::new(None::<u8>));
    let add = |label: &str, sub: &str, id: u8| {
        let row = adw::ButtonRow::builder().title(label).build();
        if !sub.is_empty() {
            row.set_tooltip_text(Some(sub));
        }
        let c = choice.clone();
        let d = form.dialog.clone();
        row.connect_activated(move |_| {
            c.set(Some(id));
            d.close();
        });
        form.add(&row);
    };
    if let Some((kind, name)) = cfg.classify(&current) {
        add(&format!("Finish {} “{name}”", kind.label()), "", 0);
    }
    add("Start New Feature", "Branch off develop", 1);
    add("Start New Release", "Branch off develop", 2);
    add("Start New Hotfix", "Branch off the production branch", 3);
    form.ok.set_visible(false);
    form.run(&rv.widget).await;
    match choice.get() {
        Some(0) => finish(rv, &current).await,
        Some(1) => start(rv, &cfg, FlowKind::Feature).await,
        Some(2) => start(rv, &cfg, FlowKind::Release).await,
        Some(3) => start(rv, &cfg, FlowKind::Hotfix).await,
        _ => {}
    }
}

async fn init(rv: &Rc<RepoView>) {
    let snap = rv.snapshot();
    let d = FlowConfig::default();
    let locals: Vec<String> = snap.refs.locals().map(|r| r.name.clone()).collect();
    let master_default = if locals.iter().any(|l| l == "main") {
        "main"
    } else if locals.iter().any(|l| l == "master") {
        "master"
    } else {
        "main"
    };
    let form = Form::new("Initialise Git-flow", "OK");
    form.description("Git-flow is not set up in this repository yet. Choose the branch names and prefixes to use.");
    form.group("Branches");
    let master = form.entry("Production branch", master_default);
    let develop = form.entry("Development branch", &d.develop);
    form.group("Prefixes");
    let feature = form.entry("Feature", &d.feature);
    let release = form.entry("Release", &d.release);
    let hotfix = form.entry("Hotfix", &d.hotfix);
    let versiontag = form.entry("Version tag prefix", "");
    if !form.run(&rv.widget).await {
        return;
    }
    let cfg = FlowConfig {
        master: master.text().trim().to_string(),
        develop: develop.text().trim().to_string(),
        feature: feature.text().trim().to_string(),
        release: release.text().trim().to_string(),
        hotfix: hotfix.text().trim().to_string(),
        support: d.support,
        versiontag: versiontag.text().trim().to_string(),
    };
    let cmds = cfg.init_commands(
        locals.contains(&cfg.master),
        locals.contains(&cfg.develop),
        snap.refs.head_oid.is_some(),
    );
    rv.run_ops("Initialise Git-flow", cmds, OpOptions::default()).await;
}

async fn start(rv: &Rc<RepoView>, cfg: &FlowConfig, kind: FlowKind) {
    let form = Form::new(&format!("New {}", kind.label()), &format!("Start {}", kind.label()));
    let name = form.entry(&format!("{} name", kind.label()), "");
    let default_base = match kind {
        FlowKind::Hotfix => cfg.master.clone(),
        _ => cfg.develop.clone(),
    };
    let mut bases = vec![default_base.clone()];
    bases.extend(
        rv.snapshot()
            .refs
            .locals()
            .map(|r| r.name.clone())
            .filter(|n| *n != default_base),
    );
    let base = form.combo("Start at", &bases, Some(&default_base));
    form.info("Branch prefix", cfg.prefix(kind));
    let n2 = name.clone();
    form.watch(&name);
    let git = rv.git.clone();
    let prefix = cfg.prefix(kind).to_string();
    form.validate(move || {
        let t = n2.text().trim().to_string();
        !t.is_empty() && git.check(&["check-ref-format", "--branch", &format!("{prefix}{t}")])
    });
    if !form.run(&rv.widget).await {
        return;
    }
    let cmds = cfg.start_commands(kind, name.text().trim(), Some(&combo_value(&base)));
    rv.run_ops(&format!("Start {}", kind.label()), cmds, OpOptions::default()).await;
}

pub async fn finish(rv: &Rc<RepoView>, branch: &str) {
    let Some(cfg) = rv.snapshot().flow.clone() else { return };
    let Some((kind, name)) = cfg.classify(branch) else {
        rv.toast("This is not a git-flow feature, release or hotfix branch");
        return;
    };
    let form = Form::new(&format!("Finish {}", kind.label()), &format!("Finish {}", kind.label()));
    form.info(kind.label(), &name);
    let delete = form.switch("Delete branch", "", true);
    let rebase = form.switch("Rebase on development branch", "", false);
    rebase.set_visible(kind == FlowKind::Feature);
    let push = form.switch("Push changes to origin", "", false);
    push.set_visible(!rv.snapshot().remotes.is_empty());
    let no_tag = form.switch("Don't create a tag", "", false);
    no_tag.set_visible(kind != FlowKind::Feature);
    let msg = form.text("Tag message", &format!("{} {name}", kind.label()), 50);
    if let Some(g) = msg.parent().and_then(|p| p.parent()) { g.set_visible(kind != FlowKind::Feature) }
    if !form.run(&rv.widget).await {
        return;
    }
    let cmds = cfg.finish_commands(
        kind,
        &name,
        &FinishOptions {
            delete_branch: delete.is_active(),
            rebase: rebase.is_active(),
            push: push.is_active(),
            no_tag: no_tag.is_active(),
            tag_message: text_of(&msg),
        },
    );
    rv.run_ops(
        &format!("Finish {}", kind.label()),
        cmds,
        OpOptions {
            network: push.is_active(),
            ..Default::default()
        },
    )
    .await;
}

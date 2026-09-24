//! Native git-flow implementation, compatible with git-flow AVH's
//! `gitflow.*` configuration keys.

use super::Git;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowConfig {
    pub master: String,
    pub develop: String,
    pub feature: String,
    pub release: String,
    pub hotfix: String,
    pub support: String,
    pub versiontag: String,
}

impl Default for FlowConfig {
    fn default() -> Self {
        Self {
            master: "main".into(),
            develop: "develop".into(),
            feature: "feature/".into(),
            release: "release/".into(),
            hotfix: "hotfix/".into(),
            support: "support/".into(),
            versiontag: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    Feature,
    Release,
    Hotfix,
}

impl FlowKind {
    pub fn label(self) -> &'static str {
        match self {
            FlowKind::Feature => "Feature",
            FlowKind::Release => "Release",
            FlowKind::Hotfix => "Hotfix",
        }
    }
}

pub type Commands = Vec<Vec<String>>;

fn cmd(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn get(git: &Git, key: &str) -> Option<String> {
    git.run(&["config", "--get", key])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() || key.ends_with("versiontag"))
}

/// Returns the configuration if the repository has been initialised for git-flow.
pub fn config(git: &Git) -> Option<FlowConfig> {
    let develop = get(git, "gitflow.branch.develop")?;
    let d = FlowConfig::default();
    Some(FlowConfig {
        master: get(git, "gitflow.branch.master").unwrap_or(d.master),
        develop,
        feature: get(git, "gitflow.prefix.feature").unwrap_or(d.feature),
        release: get(git, "gitflow.prefix.release").unwrap_or(d.release),
        hotfix: get(git, "gitflow.prefix.hotfix").unwrap_or(d.hotfix),
        support: get(git, "gitflow.prefix.support").unwrap_or(d.support),
        versiontag: get(git, "gitflow.prefix.versiontag").unwrap_or_default(),
    })
}

impl FlowConfig {
    pub fn prefix(&self, kind: FlowKind) -> &str {
        match kind {
            FlowKind::Feature => &self.feature,
            FlowKind::Release => &self.release,
            FlowKind::Hotfix => &self.hotfix,
        }
    }

    /// Which flow branch (if any) `branch` is.
    pub fn classify(&self, branch: &str) -> Option<(FlowKind, String)> {
        for kind in [FlowKind::Feature, FlowKind::Release, FlowKind::Hotfix] {
            let p = self.prefix(kind);
            if !p.is_empty()
                && let Some(name) = branch.strip_prefix(p) {
                    return Some((kind, name.to_string()));
                }
        }
        None
    }

    pub fn init_commands(&self, has_master: bool, has_develop: bool, has_commits: bool) -> Commands {
        let mut c = vec![
            cmd(&["config", "gitflow.branch.master", &self.master]),
            cmd(&["config", "gitflow.branch.develop", &self.develop]),
            cmd(&["config", "gitflow.prefix.feature", &self.feature]),
            cmd(&["config", "gitflow.prefix.release", &self.release]),
            cmd(&["config", "gitflow.prefix.hotfix", &self.hotfix]),
            cmd(&["config", "gitflow.prefix.support", &self.support]),
            cmd(&["config", "gitflow.prefix.versiontag", &self.versiontag]),
        ];
        if !has_commits {
            c.push(cmd(&["checkout", "-q", "-b", &self.master]));
            c.push(cmd(&["commit", "--allow-empty", "-q", "-m", "Initial commit"]));
        } else if !has_master {
            c.push(cmd(&["branch", &self.master]));
        }
        if !has_develop {
            c.push(cmd(&["branch", "--no-track", &self.develop, &self.master]));
        }
        c.push(cmd(&["checkout", "-q", &self.develop]));
        c
    }

    pub fn start_commands(&self, kind: FlowKind, name: &str, base: Option<&str>) -> Commands {
        let branch = format!("{}{}", self.prefix(kind), name);
        let base = base.map(str::to_string).unwrap_or_else(|| match kind {
            FlowKind::Hotfix => self.master.clone(),
            _ => self.develop.clone(),
        });
        vec![cmd(&["checkout", "-b", &branch, &base])]
    }

    pub fn finish_commands(&self, kind: FlowKind, name: &str, opts: &FinishOptions) -> Commands {
        let branch = format!("{}{}", self.prefix(kind), name);
        let mut c = Vec::new();
        match kind {
            FlowKind::Feature => {
                if opts.rebase {
                    c.push(cmd(&["rebase", &self.develop, &branch]));
                }
                c.push(cmd(&["checkout", &self.develop]));
                c.push(cmd(&["merge", "--no-ff", "--no-edit", &branch]));
            }
            FlowKind::Release | FlowKind::Hotfix => {
                let tag = format!("{}{}", self.versiontag, name);
                c.push(cmd(&["checkout", &self.master]));
                c.push(cmd(&["merge", "--no-ff", "--no-edit", &branch]));
                if !opts.no_tag {
                    let msg = if opts.tag_message.trim().is_empty() {
                        format!("{} {}", kind.label(), name)
                    } else {
                        opts.tag_message.clone()
                    };
                    c.push(cmd(&["tag", "-a", &tag, "-m", &msg]));
                }
                c.push(cmd(&["checkout", &self.develop]));
                c.push(cmd(&["merge", "--no-ff", "--no-edit", &branch]));
                if opts.push {
                    c.push(cmd(&["push", "origin", &self.master]));
                    if !opts.no_tag {
                        c.push(cmd(&["push", "origin", &tag]));
                    }
                }
            }
        }
        if opts.push {
            c.push(cmd(&["push", "origin", &self.develop]));
        }
        if opts.delete_branch {
            c.push(cmd(&["branch", "-d", &branch]));
        }
        c
    }
}

#[derive(Debug, Clone, Default)]
pub struct FinishOptions {
    pub delete_branch: bool,
    pub rebase: bool,
    pub push: bool,
    pub no_tag: bool,
    pub tag_message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_and_commands() {
        let cfg = FlowConfig::default();
        assert_eq!(
            cfg.classify("feature/login"),
            Some((FlowKind::Feature, "login".into()))
        );
        assert_eq!(cfg.classify("main"), None);
        let s = cfg.start_commands(FlowKind::Hotfix, "1.0.1", None);
        assert_eq!(s[0], cmd(&["checkout", "-b", "hotfix/1.0.1", "main"]));
        let f = cfg.finish_commands(
            FlowKind::Release,
            "1.0",
            &FinishOptions {
                delete_branch: true,
                ..Default::default()
            },
        );
        assert!(f.contains(&cmd(&["tag", "-a", "1.0", "-m", "Release 1.0"])));
        assert_eq!(f.last().unwrap(), &cmd(&["branch", "-d", "release/1.0"]));
    }
}

//! Persistent settings and repository bookmarks, stored as JSON in
//! `$XDG_CONFIG_HOME/gitree/`.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bookmark {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub group: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomAction {
    pub name: String,
    /// Program to run.
    pub command: String,
    /// Arguments; `$REPO`, `$SHA` and `$FILE` are substituted.
    pub args: String,
    #[serde(default)]
    pub show_output: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub bookmarks: Vec<Bookmark>,
    /// Repositories open as tabs when the app was last closed.
    pub open_tabs: Vec<PathBuf>,
    pub default_clone_dir: Option<PathBuf>,
    pub diff_context: u32,
    pub diff_ignore_whitespace: bool,
    pub diff_font: String,
    pub diff_tool: String,
    pub merge_tool: String,
    pub terminal: String,
    pub fetch_interval_min: u32,
    pub log_page_size: u32,
    pub confirm_dangerous: bool,
    pub push_after_commit: bool,
    pub file_tree_view: bool,
    pub custom_actions: Vec<CustomAction>,
    /// Recent commit messages per repository path.
    pub commit_history: HashMap<String, Vec<String>>,
    pub window_width: i32,
    pub window_height: i32,
    pub window_maximized: bool,
    pub progress_width: i32,
    pub progress_height: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bookmarks: Vec::new(),
            open_tabs: Vec::new(),
            default_clone_dir: dirs::home_dir(),
            diff_context: 3,
            diff_ignore_whitespace: false,
            diff_font: "Monospace 10".into(),
            diff_tool: "meld".into(),
            merge_tool: "meld".into(),
            terminal: String::new(),
            fetch_interval_min: 10,
            log_page_size: 1500,
            confirm_dangerous: true,
            push_after_commit: false,
            file_tree_view: false,
            custom_actions: Vec::new(),
            commit_history: HashMap::new(),
            window_width: 1280,
            window_height: 820,
            window_maximized: false,
            progress_width: 720,
            progress_height: 460,
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gitree")
        .join("settings.json")
}

impl Settings {
    fn load() -> Self {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }

    pub fn add_bookmark(&mut self, path: PathBuf) {
        if self.bookmarks.iter().any(|b| b.path == path) {
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        self.bookmarks.push(Bookmark {
            name,
            path,
            group: String::new(),
        });
    }

    pub fn remember_message(&mut self, repo: &str, msg: &str) {
        let list = self.commit_history.entry(repo.to_string()).or_default();
        list.retain(|m| m != msg);
        list.insert(0, msg.to_string());
        list.truncate(20);
    }
}

thread_local! {
    static SETTINGS: Rc<RefCell<Settings>> = Rc::new(RefCell::new(Settings::load()));
}

/// Read access to the settings.
pub fn with<R>(f: impl FnOnce(&Settings) -> R) -> R {
    SETTINGS.with(|s| f(&s.borrow()))
}

/// Mutates and immediately saves the settings.
pub fn update(f: impl FnOnce(&mut Settings)) {
    SETTINGS.with(|s| {
        f(&mut s.borrow_mut());
        s.borrow().save();
    });
}

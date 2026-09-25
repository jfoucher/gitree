//! Running programs on the host system.
//!
//! Native builds spawn programs directly. Inside a Flatpak sandbox, git,
//! terminals and custom actions must run on the host so that hooks,
//! credential helpers, ssh/gpg agents and LFS behave as in a terminal; there
//! commands are routed through `flatpak-spawn --host`.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// True when running inside a Flatpak sandbox.
pub fn in_flatpak() -> bool {
    static FLATPAK: OnceLock<bool> = OnceLock::new();
    *FLATPAK.get_or_init(|| std::path::Path::new("/.flatpak-info").exists())
}

/// Converts `cmd` so it runs on the host. Its program, arguments,
/// environment changes and working directory carry over; stdio must be
/// configured afterwards. With `tied`, the host process is killed when the
/// returned command exits or is killed (use it for git, so cancelling works),
/// otherwise it may outlive Gitree (terminals).
pub fn wrap(cmd: Command, tied: bool) -> Command {
    if !in_flatpak() {
        return cmd;
    }
    let mut host = Command::new("flatpak-spawn");
    host.arg("--host");
    if tied {
        host.arg("--watch-bus");
    }
    if let Some(dir) = cmd.get_current_dir() {
        let mut a = std::ffi::OsString::from("--directory=");
        a.push(dir);
        host.arg(a);
        // flatpak-spawn itself must also start somewhere valid.
        host.current_dir(dir);
    }
    for (k, v) in cmd.get_envs() {
        let mut a = std::ffi::OsString::from(if v.is_some() { "--env=" } else { "--unset-env=" });
        a.push(k);
        if let Some(v) = v {
            a.push("=");
            a.push(v);
        }
        host.arg(a);
    }
    host.arg(cmd.get_program());
    host.args(cmd.get_args());
    host
}

/// Whether `program` can be found on the host's PATH.
pub fn has_program(program: &str) -> bool {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "command -v \"$1\" >/dev/null", "sh", program]);
    wrap(cmd, true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Program the host's git should run as GIT_ASKPASS/SSH_ASKPASS to reach
/// Gitree's credential dialog. Natively that's our own executable; in a
/// Flatpak it's a small script in the app's data dir (the same path on the
/// host) that re-enters the sandbox.
pub fn askpass_program() -> Option<PathBuf> {
    if !in_flatpak() {
        return std::env::current_exe().ok();
    }
    static SCRIPT: OnceLock<Option<PathBuf>> = OnceLock::new();
    SCRIPT
        .get_or_init(|| {
            use std::os::unix::fs::PermissionsExt;
            let app_id = std::env::var("FLATPAK_ID").unwrap_or_else(|_| crate::APP_ID.into());
            let path = dirs::data_dir()?.join("gitree-askpass");
            let script = format!(
                "#!/bin/sh\nexec flatpak run --command=gitree --env={}=1 {app_id} \"$@\"\n",
                crate::ASKPASS_ENV
            );
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::write(&path, script).ok()?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).ok()?;
            Some(path)
        })
        .clone()
}

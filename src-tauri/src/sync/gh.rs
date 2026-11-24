//! GitHub CLI: sign-in status. fleet never stores tokens; `gh auth setup-git` makes plain
//! `git push` use gh's credentials.
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GhStatus {
    pub installed: bool,
    pub authed: bool,
    pub user: Option<String>,
    pub message: Option<String>,
}

pub fn bin() -> Option<std::path::PathBuf> {
    crate::shell_env::which("gh")
}

pub fn status() -> GhStatus {
    let Some(gh) = bin() else { return GhStatus { message: Some("GitHub CLI (gh) isn't installed.".into()), ..Default::default() } };
    let out = Command::new(gh).args(["auth", "status", "--hostname", "github.com"]).env_clear().envs(crate::shell_env::login_env()).output();
    match out {
        Ok(o) => {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            let user = text.lines().find_map(|l| l.split(" account ").nth(1).map(|r| r.split_whitespace().next().unwrap_or("").to_string())).filter(|u| !u.is_empty());
            GhStatus { installed: true, authed: o.status.success(), user, message: (!o.status.success()).then(|| "Not signed in to GitHub.".into()) }
        }
        Err(e) => GhStatus { installed: true, authed: false, user: None, message: Some(e.to_string()) },
    }
}

/// Shell command for the sign-in / install terminal.
pub fn terminal_command(action: &str) -> Option<&'static str> {
    match action {
        "install" => Some("brew install gh && echo && echo '✓ gh installed. Close this and click Sign in.'"),
        "login" => Some("gh auth login --hostname github.com --git-protocol https --web && gh auth setup-git && echo && echo '✓ Signed in. You can close this.'"),
        _ => None,
    }
}

pub fn is_github(url: &str) -> bool {
    url.contains("github.com")
}

//! Sessions in their own git worktree: <root>/.fleet/worktrees/<slug> on branch fleet/<slug>.
use crate::{processes, Fleet};
use std::path::Path;
use std::process::Command;
use tauri::{AppHandle, Emitter, Manager};

/// "Fix the Pix fee labels!" → "fix-the-pix-fee-labels" (max 40 chars), unique in `existing`.
pub fn slugify(title: &str, existing: &dyn Fn(&str) -> bool) -> String {
    let mut s: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    s.truncate(40);
    let s = s.trim_end_matches('-').to_string();
    let base = if s.is_empty() { "session".to_string() } else { s };
    if !existing(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|c| !existing(c)).unwrap()
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).envs(crate::shell_env::login_env()).env("LC_MESSAGES", "C").output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(crate::git::explain_error(&String::from_utf8_lossy(&out.stderr), args[0]).message)
    }
}

/// Create the worktree; returns its path.
pub fn create(root: &Path, title: &str) -> Result<String, String> {
    git(root, &["rev-parse", "--verify", "HEAD"]).map_err(|_| "This project needs at least one commit before sessions can use worktrees.".to_string())?;
    let base = root.join(".fleet").join("worktrees");
    let branches = git(root, &["branch", "--format=%(refname:short)"]).unwrap_or_default();
    let slug = slugify(title, &|c| base.join(c).exists() || branches.lines().any(|b| b == format!("fleet/{c}")));
    let path = base.join(&slug);
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    processes::exclude_locally(root, ".fleet/").map_err(|e| e.to_string())?;
    git(root, &["worktree", "add", &path.to_string_lossy(), "-b", &format!("fleet/{slug}")])?;
    Ok(path.to_string_lossy().into())
}

/// A synced session's worktree: detached at `base` (origin's main). Its branch is created and
/// named from the session's title when its first prompt finishes.
pub fn create_detached(root: &Path, title: &str, base: &str) -> Result<String, String> {
    let base_dir = root.join(".fleet").join("worktrees");
    let slug = slugify(title, &|c| base_dir.join(c).exists());
    let path = base_dir.join(&slug);
    std::fs::create_dir_all(&base_dir).map_err(|e| e.to_string())?;
    processes::exclude_locally(root, ".fleet/").map_err(|e| e.to_string())?;
    git(root, &["worktree", "add", "--detach", &path.to_string_lossy(), base])?;
    Ok(path.to_string_lossy().into())
}

/// Closing a worktree session: "keep" | "merge" (into the main checkout's branch, then remove) | "delete".
#[tauri::command]
pub fn worktree_finish(app: AppHandle, session_id: String, action: String) -> Result<(), String> {
    let f = app.state::<Fleet>();
    let (wt, root) = {
        let store = f.store.lock();
        let s = store.sessions.iter().find(|s| s.id == session_id).ok_or("unknown session")?;
        let p = store.projects.iter().find(|p| p.id == s.project_id).ok_or("unknown project")?;
        (s.worktree_path.clone().ok_or("not a worktree session")?, p.root_path.clone())
    };
    let root = Path::new(&root);
    if action == "merge" && crate::sync::pusher::is_synced(root) {
        return Err("This project syncs with GitHub: branches reach main through Ship (⌘⇧D).".into());
    }
    let branch = git(Path::new(&wt), &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default().trim().to_string();
    match action.as_str() {
        "keep" => return Ok(()),
        "merge" => {
            if !git(Path::new(&wt), &["status", "--porcelain"])?.trim().is_empty() {
                return Err("The worktree has uncommitted changes. Commit them first (git panel), then merge.".into());
            }
            git(root, &["merge", "--no-edit", &branch])?;
            git(root, &["worktree", "remove", &wt])?;
            let _ = git(root, &["branch", "-d", &branch]);
        }
        "delete" => {
            git(root, &["worktree", "remove", "--force", &wt])?;
            let _ = git(root, &["branch", "-D", &branch]);
        }
        _ => return Err("unknown action".into()),
    }
    let _ = app.emit("git-changed", root.to_string_lossy().to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("Fix the Pix fee labels!", &|_| false), "fix-the-pix-fee-labels");
        assert_eq!(slugify("***", &|_| false), "session");
        assert_eq!(slugify("a", &|c| c == "a" || c == "a-2"), "a-3");
        assert!(slugify(&"x".repeat(80), &|_| false).len() <= 40);
    }

    #[test]
    fn creates_real_worktree() {
        let d = std::env::temp_dir().join(format!("fleet-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let ok = Command::new("/bin/sh").arg("-c").arg("git init -q -b main && echo a > a && git add a && git -c user.name=t -c user.email=t@t commit -qm i").current_dir(&d).status().unwrap();
        assert!(ok.success());
        let p = create(&d, "Pix fee labels").unwrap();
        assert!(p.ends_with(".fleet/worktrees/pix-fee-labels"));
        assert!(Path::new(&p).join("a").exists());
        let ex = std::fs::read_to_string(d.join(".git/info/exclude")).unwrap();
        assert!(ex.contains(".fleet/"));
        assert_eq!(create(&d, "Pix fee labels").unwrap().rsplit('/').next().unwrap(), "pix-fee-labels-2");
        let _ = std::fs::remove_dir_all(&d);
    }
}

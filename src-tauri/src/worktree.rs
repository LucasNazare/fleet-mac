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

/// Ignored things a fresh worktree lacks but needs to run: `.env*` files (copied) and
/// `node_modules` folders (APFS clones: instant, nearly free on disk, and independent, so no
/// session links its worktree to the main checkout's — such links got committed and broke every
/// checkout). Only fills what's missing; never touches anything already in the worktree.
/// Returns the paths it added.
pub fn prepare(root: &Path, wt: &Path) -> Vec<String> {
    static BUSY: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    if root == wt || !wt.is_dir() {
        return vec![];
    }
    let list = git(root, &["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"]).unwrap_or_default();
    let mut added = vec![];
    for entry in list.split('\0').filter(|e| !e.is_empty() && !e.starts_with(".fleet/")) {
        let rel = entry.trim_end_matches('/');
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let (src, dst) = (root.join(rel), wt.join(rel));
        let Ok(lmeta) = std::fs::symlink_metadata(&src) else { continue };
        if std::fs::symlink_metadata(&dst).is_ok() {
            continue;
        }
        // The worktree's own ignore rules (GitHub's .gitignore) must cover it too, or the
        // next Stop would commit the copy.
        let probe = if std::fs::metadata(&src).map(|m| m.is_dir()).unwrap_or(false) { format!("{rel}/") } else { rel.to_string() };
        if git(wt, &["check-ignore", "-q", "--no-index", "--", &probe]).is_err() {
            continue;
        }
        let is_env = name.starts_with(".env");
        // A linked node_modules is never followed; a linked .env is copied as its contents.
        if lmeta.file_type().is_symlink() && !is_env {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&src) else { continue };
        let ok = if name == "node_modules" && meta.is_dir() {
            // Dependencies installed for other lockfiles are worse than none: setup installs.
            if !same_lockfiles(src.parent().unwrap_or(root), dst.parent().unwrap_or(wt)) {
                continue;
            }
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let ok = Command::new("/bin/cp").arg("-cR").arg(&src).arg(&dst).status().map(|s| s.success()).unwrap_or(false);
            if !ok {
                let _ = std::fs::remove_dir_all(&dst);
            }
            ok
        } else if is_env && meta.is_file() {
            dst.parent().map(|p| std::fs::create_dir_all(p).is_ok()).unwrap_or(false) && std::fs::copy(&src, &dst).is_ok()
        } else {
            false
        };
        if ok {
            added.push(rel.to_string());
        }
    }
    added
}

/// Every lockfile next to node_modules is byte-identical in both folders (missing in both
/// counts as identical).
fn same_lockfiles(a: &Path, b: &Path) -> bool {
    ["package-lock.json", "npm-shrinkwrap.json", "pnpm-lock.yaml", "yarn.lock", "bun.lockb", "bun.lock"].iter().all(|f| std::fs::read(a.join(f)).ok() == std::fs::read(b.join(f)).ok())
}

fn prepare_in_background(root: &Path, wt: &Path) {
    let (root, wt) = (root.to_path_buf(), wt.to_path_buf());
    std::thread::spawn(move || prepare(&root, &wt));
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
    // Entries for worktree folders deleted by hand would block the add.
    let _ = git(root, &["worktree", "prune"]);
    git(root, &["worktree", "add", &path.to_string_lossy(), "-b", &format!("fleet/{slug}")])?;
    prepare_in_background(root, &path);
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
    let _ = git(root, &["worktree", "prune"]);
    git(root, &["worktree", "add", "--detach", &path.to_string_lossy(), base])?;
    prepare_in_background(root, &path);
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

    #[test]
    fn prepare_fills_ignored_env_and_node_modules() {
        let d = std::env::temp_dir().join(format!("fleet-prep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("web/node_modules/vite")).unwrap();
        std::fs::create_dir_all(d.join("api/node_modules")).unwrap();
        let sh = |c: &str| assert!(Command::new("/bin/sh").arg("-c").arg(c).current_dir(&d).status().unwrap().success());
        sh("git init -q -b main && printf 'node_modules\\n.env*\\n' > .gitignore && touch web/a api/a && git add -A && git -c user.name=t -c user.email=t@t commit -qm i");
        std::fs::write(d.join("web/node_modules/vite/index.js"), "v").unwrap();
        std::fs::write(d.join("api/.env"), "K=1").unwrap();
        std::fs::write(d.join(".env.local"), "L=1").unwrap();
        let wt = d.join("wt");
        sh("git worktree add -q --detach wt");
        std::fs::write(wt.join(".env.local"), "mine").unwrap();
        let mut added = prepare(&d, &wt);
        added.sort();
        assert_eq!(added, vec!["api/.env", "api/node_modules", "web/node_modules"]);
        assert_eq!(std::fs::read_to_string(wt.join("web/node_modules/vite/index.js")).unwrap(), "v");
        assert!(!std::fs::symlink_metadata(wt.join("web/node_modules")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(wt.join(".env.local")).unwrap(), "mine", "existing files are left alone");
        assert!(prepare(&d, &wt).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn prepare_is_careful() {
        let d = std::env::temp_dir().join(format!("fleet-prep2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("web/node_modules/x")).unwrap();
        std::fs::create_dir_all(d.join("shared")).unwrap();
        let sh = |c: &str| assert!(Command::new("/bin/sh").arg("-c").arg(c).current_dir(&d).status().unwrap().success());
        sh("git init -q -b main && printf 'node_modules/\\n.env\\nscratch/\\n' > .gitignore && echo v1 > web/package-lock.json && git add -A && git -c user.name=t -c user.email=t@t commit -qm i");
        std::fs::write(d.join("shared/env"), "K=1").unwrap();
        std::os::unix::fs::symlink(d.join("shared/env"), d.join(".env")).unwrap();
        // Ignored only by the main folder's uncommitted rule: never copied.
        std::fs::create_dir_all(d.join("scratch")).unwrap();
        std::fs::write(d.join(".git/info/exclude"), "local-only/\n").unwrap();
        std::fs::create_dir_all(d.join("local-only/node_modules")).unwrap();
        let wt = d.join("wt");
        sh("git worktree add -q --detach wt");
        // The worktree's lockfile differs: its deps must be installed, not cloned.
        std::fs::write(wt.join("web/package-lock.json"), "v2\n").unwrap();
        let added = prepare(&d, &wt);
        assert_eq!(added, vec![".env"], "{added:?}");
        assert!(!std::fs::symlink_metadata(wt.join(".env")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "K=1");
        let _ = std::fs::remove_dir_all(&d);
    }
}

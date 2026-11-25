//! "Create GitHub repo": a project with no `origin` gets one in a click (Ship / GitHub sync
//! dialog). The first commit is made on main if there's none (secrets left out, like the
//! session autocommit), `gh repo create` makes the repo and sets `origin`, and the branch goes
//! up with `ship::first_push` — the only main push outside Ship, and only into an empty repo.
use super::*;
use std::process::Command;

const DEFAULT_GITIGNORE: &str = "node_modules/\n.pnpm-store/\ndist/\nbuild/\nout/\n.next/\n.nuxt/\n.svelte-kit/\n.turbo/\n.cache/\ncoverage/\ntarget/\n.venv/\nvenv/\n__pycache__/\n*.pyc\n.DS_Store\n.env\n.env.*\n!.env.example\n*.log\n";

/// GitHub repo names: letters, digits, `-`, `_`, `.`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 100 && name != "." && name != ".." && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// No commits yet: commit everything on `main`. Returns the files left out (.env, keys, huge files).
pub fn first_commit(git: &Git) -> Result<Vec<String>, String> {
    if git.read(&["rev-parse", "--git-dir"]).is_err() {
        git.run(&["init", "-q", "-b", "main"]).map_err(|e| e.to_string())?;
    }
    if git.rev("HEAD").is_some() {
        return Ok(vec![]);
    }
    git.run(&["symbolic-ref", "HEAD", "refs/heads/main"]).map_err(|e| e.to_string())?;
    // No .gitignore yet: start with the usual dependency/build/OS folders so the first commit
    // (and every autocommit after it) doesn't carry them.
    let gi = git.root.join(".gitignore");
    if !gi.exists() {
        std::fs::write(&gi, DEFAULT_GITIGNORE).map_err(|e| e.to_string())?;
    }
    git.run(&["add", "-A"]).map_err(|e| e.to_string())?;
    let added: Vec<String> = git.read(&["diff", "--cached", "--name-only", "-z"]).unwrap_or_default().split('\0').filter(|s| !s.is_empty()).map(String::from).collect();
    if added.len() > autocommit::MAX_NEW_FILES {
        let _ = git.run(&["reset", "-q"]);
        return Err(format!("The folder has {} files, mostly in {}. Add the generated ones to .gitignore first, then create the repo.", added.len(), autocommit::biggest_dirs(&added)));
    }
    let staged: Vec<String> = git.read(&["diff", "--cached", "--name-only", "-z"]).unwrap_or_default().split('\0').filter(|s| !s.is_empty()).map(String::from).collect();
    let skipped: Vec<String> = autocommit::excluded_paths(&git.root, &staged);
    if !skipped.is_empty() {
        let mut a = vec!["rm", "-q", "--cached", "--"];
        a.extend(skipped.iter().map(|s| s.as_str()));
        git.run(&a).map_err(|e| e.to_string())?;
    }
    if staged.len() == skipped.len() {
        return Err("There's nothing to put on GitHub yet: the folder has no files to commit.".into());
    }
    let ident = git.ident_env();
    let ident: Vec<(&str, &str)> = ident.iter().map(|(k, v)| (*k, v.as_str())).collect();
    git.run_with(&["commit", "-q", "--no-verify", "-m", "Initial commit"], &ident, None, true).map_err(|e| e.to_string())?;
    Ok(skipped)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedRepo {
    pub url: String,
    pub branch: String,
    /// Files kept out of the first commit (still on disk).
    pub skipped: Vec<String>,
}

#[tauri::command]
pub async fn sync_create_repo(app: AppHandle, project_id: String, name: String, private: bool) -> Result<CreatedRepo, String> {
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let name = name.trim().to_string();
        if !valid_name(&name) {
            return Err("Use letters, numbers, - _ or . for the repo name.".into());
        }
        let ghs = gh::status();
        let gh = gh::bin().filter(|_| ghs.authed).ok_or("Sign in to GitHub first (GitHub sync dialog → Sign in).")?;
        let root = PathBuf::from(&p.root_path);
        let git = git_for(&app, &root);
        if git.has_remote("origin") {
            return Err("This project already has an origin remote.".into());
        }
        let skipped = first_commit(&git)?;
        let branch = git.current_branch().ok_or("Check out a branch first (the folder is on a detached commit).")?;
        let out = Command::new(gh)
            .args(["repo", "create", &name, if private { "--private" } else { "--public" }, "--source", ".", "--remote", "origin"])
            .current_dir(&root)
            .env_clear()
            .envs(crate::shell_env::login_env())
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if err.contains("already exists") { format!("You already have a GitHub repo named {name}. Pick another name.") } else { format!("GitHub didn't create the repo: {err}") });
        }
        let url = String::from_utf8_lossy(&out.stdout).lines().find(|l| l.starts_with("https://")).unwrap_or_default().trim().to_string();
        match ship::first_push(&git, &branch) {
            pusher::PushOutcome::Pushed => {}
            o => return Err(format!("The repo was created{}, but pushing {branch} failed: {o:?}", if url.is_empty() { String::new() } else { format!(" ({url})") })),
        }
        let _ = git.run(&["fetch", "-q", "origin"]);
        let _ = git.read(&["branch", "--set-upstream-to", &format!("origin/{branch}"), &branch]);
        refresh_configs(&app, Some(&p.id));
        Ok(CreatedRepo { url, branch, skipped })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::super::gitx::testutil::sh;
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("fleet") && valid_name("my-app_2.0"));
        assert!(!valid_name("") && !valid_name("a b") && !valid_name("x/y") && !valid_name(".."));
    }

    /// A folder with no commits → first commit on main without secrets → pushed into an empty
    /// repo; a second first push (repo no longer empty) is refused.
    #[test]
    fn first_commit_and_push_into_empty_repo_only() {
        let dir = std::env::temp_dir().join(format!("fleet-repo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        sh(&dir, "git init -q --bare -b main github.git && mkdir app && cd app && git init -q -b master && git config user.name t && git config user.email t@t && echo hi > a.txt && echo SECRET=1 > .env");
        let app = dir.join("app");
        let git = Git::new(&app, None, None);
        let skipped = first_commit(&git).unwrap();
        assert!(skipped.is_empty(), "the default .gitignore keeps .env out: {skipped:?}");
        assert_eq!(git.current_branch().as_deref(), Some("main"));
        assert_eq!(git.read(&["ls-files"]).unwrap().trim(), ".gitignore\na.txt");
        assert!(app.join(".env").exists(), "kept on disk");
        assert_eq!(first_commit(&git).unwrap(), Vec::<String>::new(), "idempotent once there's a commit");
        sh(&app, "git remote add origin ../github.git");
        assert_eq!(ship::first_push(&git, "main"), pusher::PushOutcome::Pushed);
        assert!(Git::new(dir.join("github.git"), None, None).rev("main").is_some());
        assert!(matches!(ship::first_push(&git, "main"), pusher::PushOutcome::Refused(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

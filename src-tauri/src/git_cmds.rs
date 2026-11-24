//! Git commands for the right panel. Every action runs the git CLI in the
//! checkout and is recorded in the Output drawer; reads (status, diff) are not.
use crate::git::{self, FileDiff, GitError};
use crate::{shell_env, Fleet};
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

type R<T> = Result<T, GitError>;

#[derive(Default)]
pub struct GitState {
    /// Running cancellable op per repo root: child pid.
    running: Mutex<HashMap<String, u32>>,
    watchers: Mutex<HashMap<String, notify_debouncer_mini::Debouncer<notify::FsEventWatcher>>>,
    seq: std::sync::atomic::AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEntry {
    pub id: u64,
    pub root: String,
    pub cmd: String,
    pub output: String,
    pub code: i32,
    pub ms: u64,
    pub at: i64,
    /// Run by GitHub sync rather than by a click.
    #[serde(default)]
    pub auto: bool,
}

impl GitState {
    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

fn simple_err(msg: impl Into<String>) -> GitError {
    let m = msg.into();
    GitError { message: m.clone(), fix: None, fix_label: None, raw: m }
}

fn check_root(root: &str) -> R<PathBuf> {
    let p = PathBuf::from(root);
    if !p.is_dir() {
        return Err(simple_err(format!("{root} doesn't exist")));
    }
    Ok(p)
}

fn git_cmd(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(root);
    c.env_clear().envs(shell_env::login_env());
    c.env("GIT_TERMINAL_PROMPT", "0").env("LC_MESSAGES", "C").env("GIT_PAGER", "cat");
    c
}

/// Run git without recording (reads). Returns stdout; Err on non-zero unless `ok_codes` allows it.
fn read_git(root: &Path, args: &[&str], ok_codes: &[i32]) -> R<String> {
    let out = git_cmd(root).env("GIT_OPTIONAL_LOCKS", "0").args(args).stdin(Stdio::null()).output().map_err(|e| simple_err(e.to_string()))?;
    let code = out.status.code().unwrap_or(-1);
    if code == 0 || ok_codes.contains(&code) {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(git::explain_error(&String::from_utf8_lossy(&out.stderr), args.first().copied().unwrap_or("")))
    }
}

fn read_git_bytes(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = git_cmd(root).env("GIT_OPTIONAL_LOCKS", "0").args(args).stdin(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// Run a recorded git action. Cancellable via `git_cancel` when `cancellable`.
fn run_git(app: &AppHandle, root: &Path, args: &[String], stdin: Option<&str>, cancellable: bool) -> R<String> {
    let gs = &app.state::<Fleet>().git;
    let root_s = root.to_string_lossy().to_string();
    let t0 = Instant::now();
    let mut child = git_cmd(root)
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| simple_err(e.to_string()))?;
    if let Some(s) = stdin {
        let _ = child.stdin.take().unwrap().write_all(s.as_bytes());
    }
    if cancellable {
        gs.running.lock().insert(root_s.clone(), child.id());
    }
    let out = child.wait_with_output().map_err(|e| simple_err(e.to_string()));
    if cancellable {
        gs.running.lock().remove(&root_s);
    }
    let out = out?;
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let mut output = format!("{}{}", stdout, stderr);
    if output.len() > 20_000 {
        output.truncate(20_000);
        output.push_str("\n… (truncated)");
    }
    let entry = OutputEntry {
        id: gs.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        root: root_s,
        cmd: format!("git {}", args.iter().map(|a| if a.contains(' ') || a.is_empty() { format!("'{a}'") } else { a.clone() }).collect::<Vec<_>>().join(" ")),
        output: output.trim_end().to_string(),
        code,
        ms: t0.elapsed().as_millis() as u64,
        at: crate::model::now_ms(),
        auto: false,
    };
    let _ = app.emit("git-output", &entry);
    let _ = app.emit("git-changed", &entry.root);
    if code == 0 {
        Ok(stdout)
    } else if code == -1 && out.status.code().is_none() {
        Err(simple_err("Cancelled"))
    } else {
        Err(git::explain_error(&stderr, args.first().map(|s| s.as_str()).unwrap_or("")))
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

// ------------------------------------------------------------------ status & diffs

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatus {
    is_repo: bool,
    status: Option<git::Status>,
    /// Top-level of the repo (may differ from the given folder).
    top: Option<String>,
    merging: bool,
}

#[tauri::command]
pub fn git_status(root: String) -> R<RepoStatus> {
    let root = check_root(&root)?;
    let top = match read_git(&root, &["rev-parse", "--show-toplevel"], &[]) {
        Ok(t) => t.trim().to_string(),
        Err(_) => return Ok(RepoStatus { is_repo: false, status: None, top: None, merging: false }),
    };
    let text = read_git(&root, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"], &[])?;
    let mut st = git::parse_status(&text);
    let unstaged = read_git(&root, &["diff", "--numstat", "-z"], &[]).unwrap_or_default();
    let staged = read_git(&root, &["diff", "--cached", "--numstat", "-z"], &[]).unwrap_or_default();
    git::apply_numstat(&mut st, &unstaged, &staged);
    let git_dir = read_git(&root, &["rev-parse", "--git-dir"], &[]).unwrap_or_default();
    let gd = root.join(git_dir.trim());
    let merging = gd.join("MERGE_HEAD").exists() || gd.join("rebase-merge").exists() || gd.join("CHERRY_PICK_HEAD").exists();
    Ok(RepoStatus { is_repo: true, status: Some(st), top: Some(top), merging })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    original: String,
    modified: String,
    diff: FileDiff,
    binary: bool,
}

fn text_of(bytes: Option<Vec<u8>>) -> (String, bool) {
    match bytes {
        Some(b) => {
            let bin = b.iter().take(8000).any(|c| *c == 0);
            (if bin { String::new() } else { String::from_utf8_lossy(&b).into_owned() }, bin)
        }
        None => (String::new(), false),
    }
}

/// Both sides of a file's diff (HEAD↔index when `staged`, index↔worktree otherwise) + parsed hunks.
#[tauri::command]
pub fn git_diff(root: String, path: String, staged: bool) -> R<DiffResult> {
    let root = check_root(&root)?;
    let in_index = read_git(&root, &["ls-files", "--error-unmatch", "--", &path], &[]).is_ok();
    let (original, b1) = if staged {
        text_of(read_git_bytes(&root, &["show", &format!("HEAD:{path}")]))
    } else if in_index {
        text_of(read_git_bytes(&root, &["show", &format!(":{path}")]))
    } else {
        (String::new(), false)
    };
    let (modified, b2) = if staged {
        text_of(read_git_bytes(&root, &["show", &format!(":{path}")]))
    } else {
        text_of(std::fs::read(root.join(&path)).ok())
    };
    let diff_text = if staged {
        read_git(&root, &["diff", "--cached", "-U3", "--no-color", "--", &path], &[])?
    } else if in_index {
        read_git(&root, &["diff", "-U3", "--no-color", "--", &path], &[])?
    } else {
        read_git(&root, &["diff", "--no-index", "-U3", "--no-color", "--", "/dev/null", &path], &[1])?
    };
    Ok(DiffResult { original, modified, diff: git::parse_diff(&diff_text), binary: b1 || b2 })
}

/// Stage / unstage / discard one hunk, or only the selected new-file lines of it.
#[tauri::command]
pub fn git_hunk(app: AppHandle, root: String, path: String, staged: bool, hunk: usize, select: Option<(u32, u32)>, action: String) -> R<()> {
    let rootp = check_root(&root)?;
    let in_index = read_git(&rootp, &["ls-files", "--error-unmatch", "--", &path], &[]).is_ok();
    if !in_index && action == "stage" && select.is_none() {
        run_git(&app, &rootp, &s(&["add", "--", &path]), None, false)?;
        return Ok(());
    }
    let text = if staged {
        read_git(&rootp, &["diff", "--cached", "-U3", "--no-color", "--", &path], &[])?
    } else if in_index {
        read_git(&rootp, &["diff", "-U3", "--no-color", "--", &path], &[])?
    } else {
        // Untracked: make it known to the index first (intent-to-add) so partial staging works.
        run_git(&app, &rootp, &s(&["add", "-N", "--", &path]), None, false)?;
        read_git(&rootp, &["diff", "-U3", "--no-color", "--", &path], &[])?
    };
    let d = git::parse_diff(&text);
    let reverse = action != "stage";
    let patch = git::hunk_patch(&d, hunk, select, reverse).ok_or_else(|| simple_err("Nothing selected in that hunk"))?;
    let args: Vec<&str> = match action.as_str() {
        "stage" => vec!["apply", "--cached", "--recount", "-"],
        "unstage" => vec!["apply", "--cached", "--reverse", "--recount", "-"],
        "discard" => vec!["apply", "--reverse", "--recount", "-"],
        _ => return Err(simple_err("unknown action")),
    };
    run_git(&app, &rootp, &s(&args), Some(&patch), false)?;
    Ok(())
}

#[tauri::command]
pub fn git_stage(app: AppHandle, root: String, paths: Vec<String>) -> R<()> {
    let rootp = check_root(&root)?;
    let mut a = s(&["add", "-A", "--"]);
    a.extend(paths);
    run_git(&app, &rootp, &a, None, false).map(|_| ())
}

#[tauri::command]
pub fn git_unstage(app: AppHandle, root: String, paths: Vec<String>) -> R<()> {
    let rootp = check_root(&root)?;
    let has_head = read_git(&rootp, &["rev-parse", "--verify", "HEAD"], &[]).is_ok();
    let mut a = if has_head { s(&["restore", "--staged", "--"]) } else { s(&["rm", "--cached", "-r", "-q", "--"]) };
    a.extend(paths);
    run_git(&app, &rootp, &a, None, false).map(|_| ())
}

/// Discard working-tree changes. Tracked files are restored from the index;
/// untracked ones go to the Trash (never deleted outright).
#[tauri::command]
pub fn git_discard(app: AppHandle, root: String, paths: Vec<String>) -> R<()> {
    let rootp = check_root(&root)?;
    let (mut tracked, mut untracked) = (vec![], vec![]);
    for p in paths {
        if read_git(&rootp, &["ls-files", "--error-unmatch", "--", &p], &[]).is_ok() {
            tracked.push(p);
        } else {
            untracked.push(p);
        }
    }
    if !tracked.is_empty() {
        let mut a = s(&["restore", "--"]);
        a.extend(tracked);
        run_git(&app, &rootp, &a, None, false)?;
    }
    for p in untracked {
        trash::delete(rootp.join(&p)).map_err(|e| simple_err(format!("Couldn't move {p} to the Trash: {e}")))?;
    }
    let _ = app.emit("git-changed", &root);
    Ok(())
}

#[tauri::command]
pub async fn git_commit(app: AppHandle, root: String, message: String, amend: bool, push: bool) -> R<()> {
    let rootp = check_root(&root)?;
    if message.trim().is_empty() && !amend {
        return Err(simple_err("Write a commit message first."));
    }
    let mut a = s(&["commit"]);
    if amend {
        a.push("--amend".into());
        if message.trim().is_empty() {
            a.push("--no-edit".into());
        }
    }
    if !message.trim().is_empty() {
        a.push("-m".into());
        a.push(message);
    }
    if push {
        crate::sync::pusher::guard_manual(&rootp, &s(&["push"])).map_err(simple_err)?;
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        run_git(&app2, &rootp, &a, None, false)?;
        if push {
            remote_op_blocking(&app2, &rootp, "push")?;
        }
        Ok(())
    })
    .await
    .map_err(|e| simple_err(e.to_string()))?
}

fn remote_op_blocking(app: &AppHandle, root: &Path, op: &str) -> R<()> {
    let args: Vec<String> = match op {
        "fetch" => s(&["fetch", "--all", "--prune"]),
        "pull" => s(&["pull", "--no-rebase"]),
        "pull_rebase" => s(&["pull", "--rebase"]),
        "push" => s(&["push"]),
        "set_upstream_push" => {
            let branch = read_git(root, &["rev-parse", "--abbrev-ref", "HEAD"], &[])?.trim().to_string();
            let remote = read_git(root, &["remote"], &[])?.lines().next().unwrap_or("origin").to_string();
            vec!["push".into(), "-u".into(), remote, branch]
        }
        _ => return Err(simple_err("unknown operation")),
    };
    if args[0] == "push" {
        crate::sync::pusher::guard_manual(root, &args).map_err(simple_err)?;
    }
    run_git(app, root, &args, None, true).map(|_| ())
}

/// fetch / pull / pull_rebase / push / set_upstream_push — cancellable with git_cancel.
#[tauri::command]
pub async fn git_remote(app: AppHandle, root: String, op: String) -> R<()> {
    let rootp = check_root(&root)?;
    tauri::async_runtime::spawn_blocking(move || remote_op_blocking(&app, &rootp, &op)).await.map_err(|e| simple_err(e.to_string()))?
}

#[tauri::command]
pub fn git_cancel(root: String, fleet: State<Fleet>) {
    if let Some(pid) = fleet.git.running.lock().get(&root) {
        unsafe {
            libc::kill(*pid as i32, libc::SIGTERM);
        }
    }
}

const ALLOWED: &[&str] = &["checkout", "switch", "revert", "cherry-pick", "branch", "merge", "rebase", "push", "worktree", "stash", "init", "add", "restore", "reset", "tag", "remote"];

/// Run a recorded git command (subcommand must be on the allowlist).
#[tauri::command]
pub async fn git_run(app: AppHandle, root: String, args: Vec<String>) -> R<String> {
    let rootp = check_root(&root)?;
    if !args.first().map(|a| ALLOWED.contains(&a.as_str())).unwrap_or(false) {
        return Err(simple_err("That git command isn't allowed from the panel."));
    }
    if args[0] == "push" {
        crate::sync::pusher::guard_manual(&rootp, &args).map_err(simple_err)?;
    }
    tauri::async_runtime::spawn_blocking(move || run_git(&app, &rootp, &args, None, true)).await.map_err(|e| simple_err(e.to_string()))?
}

// ------------------------------------------------------------------ history, branches, stash

#[tauri::command]
pub fn git_log(root: String, limit: Option<u32>) -> R<Vec<git::Commit>> {
    let root = check_root(&root)?;
    let n = format!("-n{}", limit.unwrap_or(200));
    match read_git(&root, &["log", &n, "--date-order", git::LOG_FORMAT], &[]) {
        Ok(t) => Ok(git::parse_log(&t)),
        Err(_) => Ok(vec![]), // no commits yet
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFile {
    status: String,
    path: String,
}

#[tauri::command]
pub fn git_commit_files(root: String, hash: String) -> R<Vec<CommitFile>> {
    let root = check_root(&root)?;
    let t = read_git(&root, &["show", "--name-status", "--format=", "-z", "-M", &hash], &[])?;
    let mut out = vec![];
    let mut it = t.split('\0').filter(|x| !x.is_empty());
    while let Some(st) = it.next() {
        let st = st.trim().to_string();
        let path = if st.starts_with('R') || st.starts_with('C') {
            it.next();
            it.next()
        } else {
            it.next()
        };
        if let Some(p) = path {
            out.push(CommitFile { status: st.chars().next().unwrap_or('M').to_string(), path: p.to_string() });
        }
    }
    Ok(out)
}

#[tauri::command]
pub fn git_commit_diff(root: String, hash: String, path: String) -> R<DiffResult> {
    let root = check_root(&root)?;
    let (original, b1) = text_of(read_git_bytes(&root, &["show", &format!("{hash}^:{path}")]));
    let (modified, b2) = text_of(read_git_bytes(&root, &["show", &format!("{hash}:{path}")]));
    Ok(DiffResult { original, modified, diff: FileDiff::default(), binary: b1 || b2 })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    branches: Vec<git::Branch>,
    worktrees: Vec<git::Worktree>,
}

#[tauri::command]
pub fn git_branches(root: String) -> R<BranchInfo> {
    let root = check_root(&root)?;
    let b = read_git(&root, &["for-each-ref", "--sort=-committerdate", git::BRANCH_FORMAT, "refs/heads", "refs/remotes"], &[])?;
    let w = read_git(&root, &["worktree", "list", "--porcelain"], &[]).unwrap_or_default();
    Ok(BranchInfo { branches: git::parse_branches(&b), worktrees: git::parse_worktrees(&w) })
}

#[tauri::command]
pub fn git_stashes(root: String) -> R<Vec<git::Stash>> {
    let root = check_root(&root)?;
    Ok(git::parse_stashes(&read_git(&root, &["stash", "list", "--format=%gd%x1f%s%x1f%ct"], &[]).unwrap_or_default()))
}

/// Draft a commit message (subject ≤ 72 + short body) from the staged diff with a small model.
pub fn draft_message(rootp: &Path, model: &str) -> R<String> {
    let stat = read_git(rootp, &["diff", "--cached", "--stat"], &[])?;
    if stat.trim().is_empty() {
        return Err(simple_err("Stage some changes first."));
    }
    let mut diff = read_git(rootp, &["diff", "--cached", "-U2", "--no-color"], &[])?;
    if diff.len() > 14_000 {
        let mut cut = 14_000;
        while !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        diff.truncate(cut);
        diff.push_str("\n… (diff truncated)");
    }
    let recent = read_git(rootp, &["log", "-n8", "--format=%s"], &[]).unwrap_or_default();
    let prompt = format!(
        "Write a git commit message for the staged changes below. Match the style of the recent commit subjects if they follow a convention.\n\
         Reply with ONLY the message: a subject line of at most 72 characters, then optionally a blank line and a short body. No code fences, no quotes.\n\n\
         <recent_subjects>\n{recent}</recent_subjects>\n\n<stat>\n{stat}</stat>\n\n<diff>\n{diff}\n</diff>"
    );
    let claude = shell_env::which("claude").ok_or_else(|| simple_err("claude not found"))?;
    let msg = crate::titler::run_claude_p(&claude, model, &crate::persist::app_dir(), &prompt).map_err(|e| simple_err(e.to_string()))?;
    Ok(msg.trim().trim_matches('`').trim().to_string())
}

#[tauri::command]
pub async fn git_write_message(root: String, model: Option<String>) -> R<String> {
    let rootp = check_root(&root)?;
    tauri::async_runtime::spawn_blocking(move || draft_message(&rootp, model.as_deref().unwrap_or("haiku"))).await.map_err(|e| simple_err(e.to_string()))?
}

// ------------------------------------------------------------------ watching

fn interesting(root: &Path, p: &Path) -> (bool, bool) {
    // (refresh git, report as file change)
    let rel = p.strip_prefix(root).unwrap_or(p);
    let s = rel.to_string_lossy();
    if s.starts_with(".git/") || s == ".git" {
        let git_meta = s == ".git/index" || s == ".git/HEAD" || s.starts_with(".git/refs") || s == ".git/MERGE_HEAD" || s.ends_with("ORIG_HEAD");
        return (git_meta, false);
    }
    let noisy = ["node_modules/", "target/", ".next/", "dist/", ".fleet/worktrees/", ".turbo/", ".cache/"];
    if noisy.iter().any(|n| s.starts_with(n) || s.contains(&format!("/{n}"))) {
        return (false, false);
    }
    (true, true)
}

/// Watch a checkout: emits `git-changed` (root) at most once per second and
/// `fs-changed` {root, paths} for editor reloads and the file tree.
#[tauri::command]
pub fn git_watch(app: AppHandle, root: String, fleet: State<Fleet>) -> R<()> {
    let rootp = check_root(&root)?;
    let mut w = fleet.git.watchers.lock();
    if w.contains_key(&root) {
        return Ok(());
    }
    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let mut deb = new_debouncer(Duration::from_millis(300), move |res: notify_debouncer_mini::DebounceEventResult| {
        if let Ok(events) = res {
            let paths: Vec<PathBuf> = events.into_iter().filter(|e| e.kind == DebouncedEventKind::Any || e.kind == DebouncedEventKind::AnyContinuous).map(|e| e.path).collect();
            let _ = tx.send(paths);
        }
    })
    .map_err(|e| simple_err(e.to_string()))?;
    deb.watcher().watch(&rootp, notify::RecursiveMode::Recursive).map_err(|e| simple_err(e.to_string()))?;
    w.insert(root.clone(), deb);
    std::thread::Builder::new()
        .name("git-watch".into())
        .spawn(move || {
            let mut last_git = Instant::now() - Duration::from_secs(5);
            let mut pending_git = false;
            loop {
                let timeout = if pending_git { Duration::from_millis(200) } else { Duration::from_secs(3600) };
                match rx.recv_timeout(timeout) {
                    Ok(paths) => {
                        let mut files = vec![];
                        for p in &paths {
                            let (g, f) = interesting(&rootp, p);
                            pending_git |= g;
                            if f {
                                files.push(p.to_string_lossy().to_string());
                            }
                        }
                        if !files.is_empty() {
                            files.truncate(300);
                            let _ = app.emit("fs-changed", serde_json::json!({ "root": root, "paths": files }));
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
                if pending_git && last_git.elapsed() >= Duration::from_secs(1) {
                    pending_git = false;
                    last_git = Instant::now();
                    let _ = app.emit("git-changed", &root);
                }
            }
        })
        .map_err(|e| simple_err(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, cmd: &str) {
        let st = Command::new("/bin/sh").arg("-c").arg(cmd).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").status().unwrap();
        assert!(st.success(), "{cmd}");
    }

    /// Real git: partial-line staging through `git apply --cached --recount`.
    #[test]
    fn stage_selected_lines_in_real_repo() {
        let dir = std::env::temp_dir().join(format!("fleet-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        sh(&dir, "git init -q -b main && printf 'a\\nb\\nc\\nd\\n' > f.txt && git add f.txt && git -c user.name=t -c user.email=t@t commit -qm init");
        std::fs::write(dir.join("f.txt"), "a\nB1\nB2\nc\nd\n").unwrap();
        let text = read_git(&dir, &["diff", "-U3", "--", "f.txt"], &[]).unwrap();
        let d = git::parse_diff(&text);
        let patch = git::hunk_patch(&d, 0, Some((2, 2)), false).unwrap();
        let mut c = git_cmd(&dir).args(["apply", "--cached", "--recount", "-"]).stdin(Stdio::piped()).spawn().unwrap();
        c.stdin.take().unwrap().write_all(patch.as_bytes()).unwrap();
        assert!(c.wait().unwrap().success());
        let staged = read_git(&dir, &["show", ":f.txt"], &[]).unwrap();
        assert_eq!(staged, "a\nB1\nc\nd\n");
        let st = git::parse_status(&read_git(&dir, &["status", "--porcelain=v2", "--branch", "-z"], &[]).unwrap());
        assert_eq!((st.files[0].index, st.files[0].worktree), ('M', 'M'));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejected_push_is_explained() {
        let base = std::env::temp_dir().join(format!("fleet-push-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        sh(&base, "git init -q --bare -b main remote.git && git clone -q remote.git a 2>/dev/null && git clone -q remote.git b 2>/dev/null");
        sh(&base.join("a"), "git checkout -q -b main && echo 1 > x && git add x && git -c user.name=t -c user.email=t@t commit -qm a && git push -q origin main");
        sh(&base.join("b"), "git checkout -q -b main && echo 2 > y && git add y && git -c user.name=t -c user.email=t@t commit -qm b");
        let out = git_cmd(&base.join("b")).args(["push", "origin", "main"]).output().unwrap();
        assert!(!out.status.success());
        let e = git::explain_error(&String::from_utf8_lossy(&out.stderr), "push");
        assert_eq!(e.fix.as_deref(), Some("pull"), "{}", e.raw);
        let _ = std::fs::remove_dir_all(&base);
    }
}

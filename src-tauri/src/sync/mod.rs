//! GitHub sync: GitHub is the source of truth. Each synced session works in its own
//! worktree on its own branch, committed and pushed after every finished prompt; fleet's
//! state lives on the `fleet-state` branch; main changes only through Ship.
pub mod adopt;
pub mod autocommit;
pub mod fleetyml;
pub mod gh;
pub mod gitx;
pub mod presence;
pub mod pusher;
pub mod redact;
pub mod load;
pub mod ship;
pub mod ship_cmds;
pub mod fix;
pub mod repo;
pub mod sleep;
pub mod state_branch;

use crate::model::{now_ms, Project, Session, SessionState, SyncInfo};
use crate::{persist, Fleet};
use fleetyml::{FleetYml, YmlError};
use gitx::{Git, LogLine, Logger};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct SyncState {
    configs: Mutex<HashMap<String, ProjectCfg>>,
    session_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Sessions whose title is being regenerated (the commit waits for it).
    pub titling: Mutex<HashSet<String>>,
    queue: Mutex<Vec<Queued>>,
    status: Mutex<GlobalStatus>,
    last_fetch: Mutex<HashMap<String, i64>>,
    fetch_soon: AtomicBool,
    flush_now: AtomicBool,
    /// Our presence tag per session (we hold its branch on GitHub).
    presence: Mutex<HashMap<String, String>>,
    presence_at: Mutex<HashMap<String, i64>>,
    /// Sessions whose next push takes the branch over from another Mac.
    take: Mutex<HashSet<String>>,
    /// Sessions on fleet-state with no local session ("On other computers"), per project.
    others: Mutex<HashMap<String, Vec<load::OtherSession>>>,
    /// Last "opened" fast-forward per session.
    opened: Mutex<HashMap<String, i64>>,
    /// Repo visibility per project (chat sync only for private repos).
    private: Mutex<HashMap<String, Option<bool>>>,
    /// Sessions told to rebase their branch onto main (Fix with Claude, Ship rebases): the
    /// commit GitHub had, so the push after the rebase may replace exactly that.
    pub rewrite: Mutex<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Queued {
    project_id: String,
    session_id: String,
    worktree: String,
    branch: String,
    attempts: u32,
    next_at: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GlobalStatus {
    pub last_sync_at: Option<i64>,
    pub offline: bool,
    pub waiting: usize,
    pub paused_all: bool,
    pub any_synced: bool,
    /// Project name → why GitHub can't be reached for a reason other than the network
    /// (signed out, no access, repo gone). Cleared by that project's next good fetch.
    pub problems: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCfg {
    pub project_id: String,
    /// Last valid config (kept while fleet.yml has errors).
    pub config: Option<FleetYml>,
    pub errors: Vec<YmlError>,
    /// "main" (origin's main) | "local" (edited here, not shipped yet) | "checkout" | "none"
    pub source: String,
    pub undeployed: bool,
    pub has_origin: bool,
    pub origin_url: String,
    pub default_branch: String,
    /// sync.enabled and an origin remote.
    pub enabled: bool,
    pub paused: bool,
    pub last_deploy: Option<state_branch::DeployEntry>,
    /// Uncommitted changes / unpushed commits in the main folder (to keep as sessions).
    pub local_changes: usize,
    pub local_ahead: u32,
}

// ---------------------------------------------------------------- helpers

pub fn machine_name() -> String {
    static M: OnceLock<String> = OnceLock::new();
    M.get_or_init(|| {
        let out = std::process::Command::new("/usr/sbin/scutil").args(["--get", "ComputerName"]).output().ok();
        out.filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| std::env::var("HOST").unwrap_or_else(|_| "this Mac".into()))
    })
    .clone()
}

fn logger(app: &AppHandle) -> Logger {
    let app = app.clone();
    Arc::new(move |l: LogLine| {
        let f = app.state::<Fleet>();
        let entry = crate::git_cmds::OutputEntry { id: f.git.next_seq(), root: l.root.clone(), cmd: l.cmd, output: l.output, code: l.code, ms: l.ms, at: now_ms(), auto: true };
        let _ = app.emit("git-output", &entry);
        let _ = app.emit("git-changed", &l.root);
    })
}

pub fn git_for(app: &AppHandle, root: impl Into<PathBuf>) -> Git {
    Git::new(root, Some(logger(app)), Some(Arc::new(crate::shell_env::login_env().clone())))
}

fn override_path(project_id: &str) -> PathBuf {
    persist::app_dir().join("fleet-config").join(format!("{project_id}.yml"))
}

/// The fleet/config-<date> branch holding this Mac's unshipped fleet.yml edit.
fn override_branch_path(project_id: &str) -> PathBuf {
    persist::app_dir().join("fleet-config").join(format!("{project_id}.branch"))
}

/// After each fetch: the unshipped fleet.yml edit's branch is pushed if it never got out
/// (saved offline), and the edit is dropped once its branch shipped or is gone everywhere —
/// so a local override can't shadow main's config forever.
fn config_upkeep(app: &AppHandle, p: &Project, git: &Git) {
    let Ok(branch) = std::fs::read_to_string(override_branch_path(&p.id)) else { return };
    let branch = branch.trim().to_string();
    let local = git.rev(&format!("refs/heads/{branch}"));
    let remote = git.rev(&format!("refs/remotes/origin/{branch}"));
    let main_ref = format!("refs/remotes/origin/{}", git.default_branch());
    let on_main = local.as_ref().is_some_and(|l| git.show_file(l, fleetyml::FILE).is_some() && git.show_file(l, fleetyml::FILE) == git.show_file(&main_ref, fleetyml::FILE));
    let shipped = on_main || (local.is_none() && state_branch::read_deploys(git, state_branch::REMOTE).iter().any(|d| d.branches.contains(&branch)));
    let drop = |why: &str| {
        let _ = std::fs::remove_file(override_path(&p.id));
        let _ = std::fs::remove_file(override_branch_path(&p.id));
        if local.is_some() && remote.is_none() {
            let _ = git.run(&["branch", "-q", "-D", &branch]);
        }
        refresh_configs(app, Some(&p.id));
        let _ = app.emit("toast", format!("{}: {why}", p.name));
    };
    if shipped && remote.is_none() {
        drop("the fleet.yml edit is on main now.");
    } else if local.is_none() && remote.is_none() {
        drop("the unshipped fleet.yml edit's branch is gone, so this Mac uses main's fleet.yml again.");
    } else if local.is_some() && remote.is_none() {
        let _ = pusher::push(git, &[pusher::Refspec::branch(&branch)], &[], None);
    }
}

fn cache_path(project_id: &str) -> PathBuf {
    persist::app_dir().join("fleet-config").join(format!("{project_id}.last-valid.json"))
}

fn paused_all() -> bool {
    crate::settings::sync_paused_all()
}

pub fn cfg(app: &AppHandle, project_id: &str) -> Option<ProjectCfg> {
    app.state::<SyncState>().configs.lock().get(project_id).cloned()
}

/// The project's config when sync is on for it (enabled, origin remote). Ignores pause.
pub fn enabled_cfg(app: &AppHandle, project_id: &str) -> Option<FleetYml> {
    cfg(app, project_id).filter(|c| c.enabled).and_then(|c| c.config)
}

fn emit_status(app: &AppHandle) {
    let st = app.state::<SyncState>();
    let waiting = st.queue.lock().len();
    let any = st.configs.lock().values().any(|c| c.enabled);
    let s = {
        let mut s = st.status.lock();
        s.waiting = waiting;
        s.paused_all = paused_all();
        s.any_synced = any;
        s.clone()
    };
    let _ = app.emit("sync-status", &s);
}

fn set_sync(app: &AppHandle, session_id: &str, f: impl FnOnce(&mut SyncInfo)) {
    crate::update_session(app, session_id, |s| {
        let mut info = s.sync.clone().unwrap_or_default();
        let before = info.clone();
        f(&mut info);
        let changed = s.sync.as_ref() != Some(&info) || before != info;
        s.sync = Some(info);
        changed
    });
}

// ---------------------------------------------------------------- fleet.yml

/// Read the effective fleet.yml for a project: this Mac's unshipped edit, else origin's main,
/// else the checkout. Invalid → keep the last valid config and report errors.
pub fn load_config(app: &AppHandle, p: &Project) -> ProjectCfg {
    let root = PathBuf::from(&p.root_path);
    let git = Git::new(&root, None, Some(Arc::new(crate::shell_env::login_env().clone())));
    let has_origin = root.is_dir() && git.has_remote("origin");
    let default_branch = if has_origin { git.default_branch() } else { "main".into() };
    let origin_url = if has_origin { git.read(&["remote", "get-url", "origin"]).unwrap_or_default().trim().to_string() } else { String::new() };
    let main_ref = format!("refs/remotes/origin/{default_branch}");
    let main_text = git.show_file(&main_ref, fleetyml::FILE).map(|b| String::from_utf8_lossy(&b).into_owned());
    let local = std::fs::read_to_string(override_path(&p.id)).ok();
    // The local edit reached main (shipped): drop it.
    if let (Some(l), Some(m)) = (&local, &main_text) {
        if l.trim() == m.trim() {
            let _ = std::fs::remove_file(override_path(&p.id));
        }
    }
    let local = std::fs::read_to_string(override_path(&p.id)).ok();
    let (text, source) = match (&local, &main_text) {
        (Some(l), _) => (Some(l.clone()), "local"),
        (None, Some(m)) => (Some(m.clone()), "main"),
        (None, None) => (std::fs::read_to_string(root.join(fleetyml::FILE)).ok(), "checkout"),
    };
    let prev = cfg(app, &p.id).and_then(|c| c.config).or_else(|| std::fs::read(cache_path(&p.id)).ok().and_then(|b| serde_json::from_slice(&b).ok()));
    let (config, errors) = match &text {
        None => (None, vec![]),
        Some(t) => match fleetyml::parse(t) {
            Ok(c) => {
                let _ = std::fs::create_dir_all(cache_path(&p.id).parent().unwrap());
                let _ = std::fs::write(cache_path(&p.id), serde_json::to_vec(&c).unwrap_or_default());
                (Some(c), vec![])
            }
            Err(e) => (prev, e),
        },
    };
    let enabled = has_origin && config.as_ref().map(|c| c.sync.enabled).unwrap_or(false);
    let last_deploy = if enabled { state_branch::read_deploys(&git, state_branch::REMOTE).into_iter().filter(|d| d.result != "redeployed").next_back() } else { None };
    let work = if enabled { adopt::scan(&git, &default_branch) } else { Default::default() };
    ProjectCfg {
        project_id: p.id.clone(),
        config,
        errors,
        source: if text.is_none() { "none".into() } else { source.into() },
        undeployed: source == "local",
        has_origin,
        origin_url,
        default_branch,
        enabled,
        paused: p.sync_paused || paused_all(),
        last_deploy,
        local_changes: work.files.len(),
        local_ahead: work.ahead,
    }
}

/// Reload every project's config; synced projects take their processes from fleet.yml.
pub fn refresh_configs(app: &AppHandle, only: Option<&str>) {
    let projects: Vec<Project> = app.state::<Fleet>().store.lock().projects.clone();
    let mut changed_processes = false;
    for p in projects.iter().filter(|p| only.map(|o| o == p.id).unwrap_or(true)) {
        let c = load_config(app, p);
        if c.enabled {
            if let Some(conf) = &c.config {
                let defs = fleetyml::to_process_defs(conf);
                let f = app.state::<Fleet>();
                let mut store = f.store.lock();
                if let Some(pp) = store.projects.iter_mut().find(|x| x.id == p.id) {
                    let same = pp.processes.len() == defs.len() && pp.processes.iter().zip(&defs).all(|(a, b)| a.id == b.id && a.command == b.command && a.cwd == b.cwd && a.env_from == b.env_from && a.auto_start == b.auto_start && a.auto_restart == b.auto_restart && a.name == b.name);
                    if !same {
                        pp.processes = defs;
                        pp.processes_offered = true;
                        changed_processes = true;
                    }
                }
            }
        }
        app.state::<SyncState>().configs.lock().insert(p.id.clone(), c);
    }
    if changed_processes {
        let f = app.state::<Fleet>();
        let store = f.store.lock();
        let _ = app.emit("projects-changed", &store.projects);
        f.mark_dirty();
    }
    let roots: Vec<PathBuf> = {
        let st = app.state::<SyncState>();
        let cfgs = st.configs.lock();
        projects.iter().filter(|p| cfgs.get(&p.id).map(|c| c.enabled).unwrap_or(false)).map(|p| PathBuf::from(&p.root_path)).collect()
    };
    pusher::set_synced_roots(roots);
    let all: Vec<ProjectCfg> = app.state::<SyncState>().configs.lock().values().cloned().collect();
    let _ = app.emit("sync-configs", &all);
    emit_status(app);
}

/// Commit fleet.yml to fleet/config-<date> (on top of origin's main, no checkout) and push it.
/// Returns the branch name.
fn commit_config_branch(app: &AppHandle, root: &Path, text: &str, cfg: &ProjectCfg) -> Result<String, String> {
    commit_settings_branch(app, root, vec![(fleetyml::FILE.into(), Some(text.as_bytes().to_vec()))], "fleet.yml: update project config", cfg)
}

/// Commit settings files (fleet.yml, workflow limits) to fleet/config-<date> and push it.
pub(crate) fn commit_settings_branch(app: &AppHandle, root: &Path, files: Vec<(String, Option<Vec<u8>>)>, subject: &str, cfg: &ProjectCfg) -> Result<String, String> {
    let git = git_for(app, root);
    let prefix = cfg.config.as_ref().map(|c| c.sync.branch_prefix.clone()).unwrap_or_else(|| "fleet/".into());
    let branch = format!("{prefix}config-{}", chrono::Local::now().format("%Y%m%d"));
    let local_ref = format!("refs/heads/{branch}");
    let main_ref = format!("refs/remotes/origin/{}", cfg.default_branch);
    let existing = git.rev(&local_ref);
    let base = existing.clone().or_else(|| git.rev(&main_ref)).or_else(|| git.rev("HEAD"));
    let msg = format!("{subject}\n\nFleet-Machine: {}\n", machine_name());
    let sha = git.commit_files(base.as_deref(), &files, &msg).map_err(|e| e.to_string())?;
    git.update_ref(&local_ref, &sha, existing.as_deref(), "fleet.yml edit").map_err(|e| e.to_string())?;
    match pusher::push(&git, &[pusher::Refspec::branch(&branch)], &[], None) {
        pusher::PushOutcome::Pushed | pusher::PushOutcome::Offline(_) => Ok(branch),
        pusher::PushOutcome::Rejected(..) => {
            // Another Mac edited the config today too: ours goes on a separate branch.
            let alt = format!("{branch}-{}", &uuid::Uuid::new_v4().to_string()[..4]);
            git.update_ref(&format!("refs/heads/{alt}"), &sha, None, "fleet.yml edit").map_err(|e| e.to_string())?;
            let _ = pusher::push(&git, &[pusher::Refspec::branch(&alt)], &[], None);
            Ok(alt)
        }
        o => Err(format!("{o:?}")),
    }
}

/// Make sure fleet-state exists locally and on GitHub.
fn ensure_state_branch(app: &AppHandle, root: &Path) {
    let git = git_for(app, root);
    if git.rev(state_branch::LOCAL).is_none() && git.rev(state_branch::REMOTE).is_none() {
        let readme = "# fleet-state\n\nfleet's database for this repo: one file per session under sessions/ and the deploy log in deploys.yml.\nThis branch is never merged or deployed. Written by fleet; safe to ignore.\n";
        let _ = state_branch::write(&git, &[("README.md".into(), Some(readme.as_bytes().to_vec()))], "state: init");
    }
    if git.rev(state_branch::LOCAL).is_some() {
        let _ = pusher::push(&git, &[pusher::Refspec::branch(state_branch::BRANCH)], &[], None);
    }
}

// ---------------------------------------------------------------- sessions

/// For new sessions in synced projects: a worktree detached at origin's main.
pub fn session_worktree(app: &AppHandle, project: &Project, title: &str) -> Result<Option<String>, String> {
    let Some(conf) = enabled_cfg(app, &project.id) else { return Ok(None) };
    let c = cfg(app, &project.id).unwrap_or_default();
    let root = PathBuf::from(&project.root_path);
    let git = Git::new(&root, None, None);
    let main_ref = format!("refs/remotes/origin/{}", c.default_branch);
    let base = if git.rev(&main_ref).is_some() { main_ref } else { "HEAD".into() };
    let path = crate::worktree::create_detached(&root, if title == "New session" { "session" } else { title }, &base)?;
    if let Some(setup) = conf.sync.setup.clone().filter(|s| !s.trim().is_empty()) {
        run_setup(app, &path, &setup);
    }
    app.state::<SyncState>().fetch_soon.store(true, Ordering::Relaxed);
    Ok(Some(path))
}

/// `sync.setup` (e.g. npm ci) in a fresh worktree, in the background; output to the Output drawer.
pub fn run_setup(app: &AppHandle, dir: &str, setup: &str) {
    let (app, dir, setup) = (app.clone(), dir.to_string(), setup.to_string());
    std::thread::spawn(move || {
        run_setup_now(&app, &dir, &setup);
    });
}

/// `sync.setup` in `dir`, on this thread; returns its exit code.
pub fn run_setup_now(app: &AppHandle, dir: &str, setup: &str) -> i32 {
    let (dir, setup) = (dir.to_string(), setup.to_string());
    {
        // The worktree's copied .env and cloned node_modules first (prepare is serialized
        // and skips what exists): setup then installs on top, never racing the clone.
        if let Ok(common) = Git::new(&dir, None, None).read(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
            if let Some(root) = Path::new(common.trim()).parent() {
                crate::worktree::prepare(root, Path::new(&dir));
            }
        }
        let t0 = std::time::Instant::now();
        let shell = crate::shell_env::login_env().get("SHELL").cloned().unwrap_or_else(|| "/bin/zsh".into());
        let out = std::process::Command::new(shell).arg("-c").arg(&setup).current_dir(&dir).env_clear().envs(crate::shell_env::login_env()).env("CI", "1").output();
        let (code, output) = match out {
            Ok(o) => (o.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
            Err(e) => (-1, e.to_string()),
        };
        let mut output = output;
        if output.len() > 20_000 {
            let mut cut = output.len() - 20_000;
            while !output.is_char_boundary(cut) {
                cut += 1;
            }
            output = format!("… {}", &output[cut..]);
        }
        logger(app)(LogLine { root: dir.clone(), cmd: format!("setup: {setup}"), output: output.trim_end().into(), code, ms: t0.elapsed().as_millis() as u64 });
        if code != 0 {
            let _ = app.emit("toast", format!("Setup ({setup}) failed in {dir} — see the Output drawer."));
        }
        code
    }
}

fn session_lock(app: &AppHandle, id: &str) -> Arc<Mutex<()>> {
    app.state::<SyncState>().session_locks.lock().entry(id.to_string()).or_default().clone()
}

fn snap_of(s: &Session) -> autocommit::Snap {
    autocommit::Snap {
        glyph: s.glyph.clone(),
        color: s.color.clone(),
        title: s.title.clone(),
        title_history: s.title_history.clone(),
        state: serde_json::to_value(s.state).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
        recap: s.recap.clone(),
        prompts: s.prompt_count,
        chat: None,
    }
}

fn find(app: &AppHandle, session_id: &str) -> Option<(Session, Project)> {
    let f = app.state::<Fleet>();
    let store = f.store.lock();
    let s = store.sessions.iter().find(|s| s.id == session_id)?.clone();
    let p = store.projects.iter().find(|p| p.id == s.project_id)?.clone();
    Some((s, p))
}

/// Stop hook: count the prompt; for synced sessions commit + state + push in the background.
pub fn on_stop(app: &AppHandle, session_id: &str) {
    crate::update_session(app, session_id, |s| {
        s.prompt_count += 1;
        true
    });
    let (app, id) = (app.clone(), session_id.to_string());
    std::thread::spawn(move || sync_session(&app, &id, false));
}

fn sync_session(app: &AppHandle, id: &str, wip: bool) {
    let Some((s, p)) = find(app, id) else { return };
    if s.remote.is_some() {
        return;
    }
    let Some(conf) = enabled_cfg(app, &p.id) else { return };
    if p.sync_paused || paused_all() {
        set_sync(app, id, |i| {
            i.badge = "paused".into();
            i.detail = Some("Sync is paused".into());
        });
        return;
    }
    let Some(wt) = s.worktree_path.clone().filter(|w| Path::new(w).is_dir()) else {
        set_sync(app, id, |i| {
            i.badge = "paused".into();
            i.detail = Some("This session works in the main checkout (started before sync was on). Start a new session to sync.".into());
        });
        return;
    };
    let lock = session_lock(app, id);
    let _g = lock.lock();
    set_sync(app, id, |i| i.badge = "syncing".into());
    let git = git_for(app, &wt);
    let model = conf.claude.title_model.clone().unwrap_or_else(crate::settings::title_model);
    let app2 = app.clone();
    let id2 = id.to_string();
    let snapshot = move || {
        // Wait (≤ 30 s) for the title + recap of this turn.
        for _ in 0..150 {
            if !app2.state::<SyncState>().titling.lock().contains(&id2) {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        find(&app2, &id2).map(|(s, _)| snap_of(&s)).unwrap_or_default()
    };
    let message = move |g: &Git| crate::git_cmds::draft_message(&g.root, &model).ok();
    let machine = machine_name();
    let chat = conf.sync.chat_sync && repo_private(app, &p) == Some(true);
    let (app3, id3, root3) = (app.clone(), id.to_string(), p.root_path.clone());
    let snapshot_chat = move || {
        let mut snap = snapshot();
        if chat {
            if let Some((s, _)) = find(&app3, &id3) {
                if let Some(t) = s.transcript_path.as_deref().and_then(|t| std::fs::read_to_string(t).ok()) {
                    let known = redact::dotenv_secrets(&[PathBuf::from(&root3), PathBuf::from(&s.cwd)]);
                    snap.chat = Some((s.claude_session_id.clone(), redact::redact(&t, &known).into_bytes()));
                }
            }
        }
        snap
    };
    let presence = Some(load::presence_ctx(app, id));
    let out = autocommit::run(&autocommit::Input { git: &git, prefix: &conf.sync.branch_prefix, machine: &machine, wip, snapshot: &snapshot_chat, message: &message, presence });
    load::remember_presence(app, id, out.presence.clone());
    apply_outcome(app, id, &p.id, &wt, &out);
}

fn apply_outcome(app: &AppHandle, id: &str, project_id: &str, wt: &str, out: &autocommit::Outcome) {
    use autocommit::Status;
    if !out.branch.is_empty() {
        crate::update_session(app, id, |s| {
            let ch = s.branch != out.branch;
            s.branch = out.branch.clone();
            ch
        });
    }
    let now = now_ms();
    match &out.status {
        Status::Pushed => {
            app.state::<SyncState>().queue.lock().retain(|q| q.session_id != id);
            let st = app.state::<SyncState>();
            // The rebased branch went out (with its lease): later pushes are plain again.
            let old = st.rewrite.lock().get(id).cloned();
            if old.is_some_and(|o| !git_for(app, wt).is_ancestor(&o, "HEAD")) {
                st.rewrite.lock().remove(id);
            }
            let mut gs = st.status.lock();
            gs.last_sync_at = Some(now);
            gs.offline = false;
        }
        Status::Waiting(_) => {
            enqueue(app, Queued { project_id: project_id.into(), session_id: id.into(), worktree: wt.into(), branch: out.branch.clone(), attempts: 0, next_at: now + 5_000 });
            app.state::<SyncState>().status.lock().offline = true;
        }
        Status::Shipped(old) => {
            app.state::<SyncState>().queue.lock().retain(|q| q.session_id != id);
            if let Some(mine) = app.state::<SyncState>().presence.lock().remove(id) {
                autocommit::release_presence(&git_for(app, wt), old, &mine);
            }
            crate::update_session(app, id, |s| {
                s.branch = String::new();
                s.files_touched.clear();
                s.line_marks.clear();
                true
            });
        }
        Status::Conflict(files) => {
            app.state::<SyncState>().queue.lock().retain(|q| q.session_id != id);
            let q = format!("Conflict with GitHub's {} in {} — resolve it (git rebase --continue), then sync resumes.", out.branch, files.join(", "));
            crate::update_session(app, id, |s| {
                s.state = SessionState::NeedsReply;
                s.state_since = now_ms();
                s.pending_question = Some(q.clone());
                true
            });
        }
        _ => {}
    }
    let waiting = app.state::<SyncState>().queue.lock().iter().filter(|q| q.session_id == id).count() as u32;
    set_sync(app, id, |i| {
        i.excluded = out.excluded.clone();
        if out.commit.is_some() {
            i.last_commit = out.commit.clone();
        }
        match &out.status {
            Status::Pushed => {
                i.badge = "pushed".into();
                i.waiting = 0;
                i.last_push_at = Some(now);
                i.detail = None;
            }
            Status::Waiting(m) => {
                i.badge = "waiting".into();
                i.waiting = waiting.max(1);
                i.detail = Some(m.clone());
            }
            Status::Conflict(f) => {
                i.badge = "conflict".into();
                i.detail = Some(format!("Conflict in {}", f.join(", ")));
            }
            Status::Paused(m) => {
                i.badge = "paused".into();
                i.detail = Some(m.clone());
            }
            Status::TakenOver(m) => {
                i.badge = "taken_over".into();
                i.detail = Some(format!("{m} — not pushed. Take over to continue here."));
            }
            Status::Error(m) => {
                i.badge = "error".into();
                i.detail = Some(m.clone());
            }
            Status::Shipped(_) => {
                i.badge = "new".into();
                i.waiting = 0;
                i.detail = Some("Shipped ✓ — the next prompt starts a new branch".into());
            }
        }
    });
    if !out.excluded.is_empty() {
        let _ = app.emit("sync-excluded", serde_json::json!({ "sessionId": id, "files": out.excluded }));
    }
    emit_status(app);
}

/// Quit / sleep while sessions are mid-prompt: wip commit + state + push, within `limit`.
pub fn wip_all(app: &AppHandle, limit: Duration) {
    let working: Vec<String> = {
        let f = app.state::<Fleet>();
        let store = f.store.lock();
        store.sessions.iter().filter(|s| s.alive && !s.archived && s.state == SessionState::Working && s.remote.is_none() && s.worktree_path.is_some()).map(|s| s.id.clone()).collect()
    };
    let working: Vec<String> = working
        .into_iter()
        .filter(|id| find(app, id).and_then(|(_, p)| enabled_cfg(app, &p.id).filter(|c| c.sync.wip_on_quit && !p.sync_paused)).is_some())
        .collect();
    if working.is_empty() || paused_all() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    for id in working.clone() {
        let (app, tx) = (app.clone(), tx.clone());
        std::thread::spawn(move || {
            sync_session(&app, &id, true);
            let _ = tx.send(());
        });
    }
    let deadline = std::time::Instant::now() + limit;
    for _ in 0..working.len() {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if rx.recv_timeout(left).is_err() {
            break;
        }
    }
    save_queue(app);
}

// ---------------------------------------------------------------- queue

fn queue_file() -> PathBuf {
    persist::app_dir().join("sync-queue.json")
}

fn save_queue(app: &AppHandle) {
    let q = app.state::<SyncState>().queue.lock().clone();
    let _ = std::fs::write(queue_file(), serde_json::to_vec_pretty(&q).unwrap_or_default());
}

fn enqueue(app: &AppHandle, item: Queued) {
    {
        let st = app.state::<SyncState>();
        let mut q = st.queue.lock();
        q.retain(|x| !(x.session_id == item.session_id && x.branch == item.branch));
        q.push(item);
    }
    save_queue(app);
}

pub fn backoff_ms(attempts: u32) -> i64 {
    (5_000i64 << attempts.min(6)).min(300_000)
}

/// Retry queued pushes that are due (or all, when `force`).
fn flush(app: &AppHandle, force: bool) {
    let due: Vec<Queued> = {
        let now = now_ms();
        app.state::<SyncState>().queue.lock().iter().filter(|q| force || q.next_at <= now).cloned().collect()
    };
    for q in due {
        if find(app, &q.session_id).map(|(_, p)| p.sync_paused).unwrap_or(false) || paused_all() {
            continue;
        }
        let lock = session_lock(app, &q.session_id);
        // That session is syncing right now (it may be waiting on its commit message): its
        // own push covers this entry; don't hold up the fetch loop for it.
        let Some(_g) = lock.try_lock() else { continue };
        if !Path::new(&q.worktree).is_dir() {
            app.state::<SyncState>().queue.lock().retain(|x| x.session_id != q.session_id);
            continue;
        }
        let git = git_for(app, &q.worktree);
        let ctx = load::presence_ctx(app, &q.session_id);
        let (status, tag) = autocommit::push_session(&git, &q.branch, Some(&ctx));
        load::remember_presence(app, &q.session_id, tag);
        let still_waiting = matches!(status, autocommit::Status::Waiting(_));
        {
            let st = app.state::<SyncState>();
            let mut queue = st.queue.lock();
            if still_waiting {
                if let Some(x) = queue.iter_mut().find(|x| x.session_id == q.session_id && x.branch == q.branch) {
                    x.attempts += 1;
                    x.next_at = now_ms() + backoff_ms(x.attempts);
                }
            } else {
                queue.retain(|x| !(x.session_id == q.session_id && x.branch == q.branch));
            }
        }
        if still_waiting {
            app.state::<SyncState>().status.lock().offline = true;
            break; // still offline: don't hammer the others now
        }
        apply_outcome(app, &q.session_id, &q.project_id, &q.worktree, &autocommit::Outcome { branch: q.branch.clone(), commit: None, excluded: vec![], status, presence: None });
    }
    save_queue(app);
    emit_status(app);
}

// ---------------------------------------------------------------- fetch loop

fn fetch_project(app: &AppHandle, p: &Project) {
    let git = git_for(app, &p.root_path);
    let ok = match git.run(&["fetch", "--prune", "origin", "+refs/heads/*:refs/remotes/origin/*", presence::FETCH_SPEC]) {
        Ok(_) => true,
        Err(e) => {
            let state = app.state::<SyncState>();
            let mut st = state.status.lock();
            if pusher::is_auth(&e.stderr) {
                st.problems.insert(p.name.clone(), "GitHub refused fleet's sign-in or this repo's access. Sign in again (GitHub sync dialog → Sign in, or `gh auth login`).".into());
            } else if pusher::is_offline(&e.stderr) {
                st.offline = true;
            } else {
                st.problems.insert(p.name.clone(), format!("Fetching from GitHub fails: {}", e.stderr.trim().lines().last().unwrap_or("git fetch failed")));
            }
            false
        }
    };
    app.state::<SyncState>().last_fetch.lock().insert(p.id.clone(), now_ms());
    if ok {
        {
            let st = app.state::<SyncState>();
            let mut s = st.status.lock();
            let was_offline = s.offline;
            s.offline = false;
            s.problems.remove(&p.name);
            s.last_sync_at = Some(now_ms());
            if was_offline {
                st.flush_now.store(true, Ordering::Relaxed);
            }
        }
        config_upkeep(app, p, &git);
        refresh_configs(app, Some(&p.id));
        load::load_sequence(app, p);
    }
    emit_status(app);
}

/// Is the project's GitHub repo private? (cached; None when unknown → no chat sync)
fn repo_private(app: &AppHandle, p: &Project) -> Option<bool> {
    if let Some(v) = app.state::<SyncState>().private.lock().get(&p.id) {
        return *v;
    }
    let v = gh::bin().and_then(|gh| {
        let out = std::process::Command::new(gh).args(["repo", "view", "--json", "isPrivate", "-q", ".isPrivate"]).current_dir(&p.root_path).env_clear().envs(crate::shell_env::login_env()).output().ok()?;
        match String::from_utf8_lossy(&out.stdout).trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    });
    app.state::<SyncState>().private.lock().insert(p.id.clone(), v);
    v
}

/// A session closed: release its presence in the background.
/// Called with the store locked: everything needed is passed in.
pub fn on_close(app: &AppHandle, s: &Session) {
    let (app, s) = (app.clone(), s.clone());
    std::thread::spawn(move || {
        // Edits from a turn that never reached Stop (interrupted, then closed): committed as
        // wip and pushed, so closing a session can't strand work in its worktree.
        if let Some(wt) = s.worktree_path.clone().filter(|w| Path::new(w).is_dir()) {
            std::thread::sleep(Duration::from_millis(1500));
            let dirty = git_for(&app, &wt).read(&["status", "--porcelain"]).map(|o| !o.trim().is_empty()).unwrap_or(false);
            if dirty && s.remote.is_none() {
                if find(&app, &s.id).is_some() {
                    sync_session(&app, &s.id, true);
                } else if let Some(conf) = project(&app, &s.project_id).ok().filter(|p| !p.sync_paused && !paused_all()).and_then(|p| enabled_cfg(&app, &p.id)) {
                    let lock = session_lock(&app, &s.id);
                    let _g = lock.lock();
                    let git = git_for(&app, &wt);
                    let snap = snap_of(&s);
                    let snapshot = || snap.clone();
                    let message = |_: &Git| None;
                    let out = autocommit::run(&autocommit::Input { git: &git, prefix: &conf.sync.branch_prefix, machine: &machine_name(), wip: true, snapshot: &snapshot, message: &message, presence: None });
                    if !matches!(out.status, autocommit::Status::Pushed | autocommit::Status::Shipped(_)) {
                        let _ = app.emit("toast", format!("{} · {}: its last edits were committed in {wt} but not pushed ({:?}).", s.glyph, s.title, out.status));
                    }
                }
            }
        }
        let branch = find(&app, &s.id).map(|(x, _)| x.branch).unwrap_or(s.branch.clone());
        let Some(mine) = app.state::<SyncState>().presence.lock().remove(&s.id) else { return };
        if let (Some(wt), false) = (s.worktree_path.clone(), branch.is_empty()) {
            autocommit::release_presence(&git_for(&app, &wt), &branch, &mine);
        }
    });
}

/// Window regained focus: fetch soon (at most every 15 s).
pub fn on_focus(app: &AppHandle) {
    app.state::<SyncState>().fetch_soon.store(true, Ordering::Relaxed);
}

pub fn start(app: AppHandle) {
    if let Ok(b) = std::fs::read(queue_file()) {
        if let Ok(q) = serde_json::from_slice::<Vec<Queued>>(&b) {
            *app.state::<SyncState>().queue.lock() = q;
        }
    }
    refresh_configs(&app, None);
    // Badges left mid-sync by the last run: queued → waiting, otherwise pushed.
    let stale: Vec<String> = app.state::<Fleet>().store.lock().sessions.iter().filter(|s| s.sync.as_ref().map(|i| i.badge == "syncing").unwrap_or(false)).map(|s| s.id.clone()).collect();
    for id in stale {
        let queued = app.state::<SyncState>().queue.lock().iter().filter(|q| q.session_id == id).count() as u32;
        set_sync(&app, &id, |i| {
            i.badge = if queued > 0 { "waiting".into() } else { "new".into() };
            i.waiting = queued;
        });
        // That sync never finished: run it again rather than claim it did.
        if queued == 0 {
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(10));
                if find(&app, &id).is_some_and(|(s, _)| s.state != SessionState::Working) {
                    sync_session(&app, &id, false);
                }
            });
        }
    }
    app.state::<SyncState>().fetch_soon.store(true, Ordering::Relaxed);
    sleep::observe(app.clone());
    std::thread::Builder::new()
        .name("github-sync".into())
        .spawn(move || loop {
            let st = app.state::<SyncState>();
            let soon = st.fetch_soon.swap(false, Ordering::Relaxed);
            let projects: Vec<Project> = app.state::<Fleet>().store.lock().projects.clone();
            let now = now_ms();
            for p in &projects {
                let Some(c) = cfg(&app, &p.id) else { continue };
                if !c.has_origin {
                    continue;
                }
                let has_cfg = c.config.is_some();
                if !c.enabled && !has_cfg {
                    continue;
                }
                let interval = c.config.as_ref().map(|c| c.sync.fetch_interval_sec).unwrap_or(120) as i64 * 1000;
                let last = st.last_fetch.lock().get(&p.id).copied().unwrap_or(0);
                if now - last >= interval || (soon && now - last >= 15_000) {
                    fetch_project(&app, p);
                }
            }
            load::refresh_presence(&app);
            if st.flush_now.swap(false, Ordering::Relaxed) {
                flush(&app, true);
            } else if !st.queue.lock().is_empty() {
                flush(&app, false);
            }
            std::thread::sleep(Duration::from_secs(2));
        })
        .expect("spawn sync loop");
}

// ---------------------------------------------------------------- commands

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncCheck {
    is_repo: bool,
    has_origin: bool,
    origin_url: String,
    is_github: bool,
    gh: gh::GhStatus,
    main_has_fleet_yml: bool,
    default_branch: String,
    enabled: bool,
    dirty_main: bool,
}

#[tauri::command]
pub fn sync_configs(app: AppHandle) -> Vec<ProjectCfg> {
    app.state::<SyncState>().configs.lock().values().cloned().collect()
}

#[tauri::command]
pub fn sync_status(app: AppHandle) -> GlobalStatus {
    emit_status(&app);
    app.state::<SyncState>().status.lock().clone()
}

fn project(app: &AppHandle, id: &str) -> Result<Project, String> {
    app.state::<Fleet>().store.lock().projects.iter().find(|p| p.id == id).cloned().ok_or_else(|| "unknown project".into())
}

#[tauri::command]
pub async fn sync_check(app: AppHandle, project_id: String) -> Result<SyncCheck, String> {
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let root = PathBuf::from(&p.root_path);
        let git = Git::new(&root, None, Some(Arc::new(crate::shell_env::login_env().clone())));
        let is_repo = git.read(&["rev-parse", "--git-dir"]).is_ok();
        let c = load_config(&app, &p);
        let main_has = git.show_file(&format!("refs/remotes/origin/{}", c.default_branch), fleetyml::FILE).is_some();
        let dirty = git.read(&["status", "--porcelain", "--untracked-files=no"]).map(|s| !s.trim().is_empty()).unwrap_or(false);
        Ok(SyncCheck {
            is_repo,
            has_origin: c.has_origin,
            is_github: gh::is_github(&c.origin_url),
            origin_url: c.origin_url.clone(),
            gh: gh::status(),
            main_has_fleet_yml: main_has,
            default_branch: c.default_branch.clone(),
            enabled: c.enabled,
            dirty_main: dirty,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The text the config editor opens with: the effective fleet.yml, or a starter.
#[tauri::command]
pub fn sync_config_text(app: AppHandle, project_id: String) -> Result<String, String> {
    let p = project(&app, &project_id)?;
    let root = PathBuf::from(&p.root_path);
    if let Ok(t) = std::fs::read_to_string(override_path(&p.id)) {
        return Ok(t);
    }
    let c = load_config(&app, &p);
    let git = Git::new(&root, None, None);
    if let Some(b) = git.show_file(&format!("refs/remotes/origin/{}", c.default_branch), fleetyml::FILE) {
        return Ok(String::from_utf8_lossy(&b).into_owned());
    }
    if let Ok(t) = std::fs::read_to_string(root.join(fleetyml::FILE)) {
        return Ok(t);
    }
    let defs = if p.processes.is_empty() { crate::processes::detect(&root) } else { p.processes.clone() };
    Ok(fleetyml::starter(&p.name, &defs, fleetyml::infer_setup(&root).as_deref(), &fleetyml::infer_checks(&root)))
}

#[tauri::command]
pub fn sync_validate(text: String) -> Vec<YmlError> {
    fleetyml::parse(&text).err().unwrap_or_default()
}

/// Save fleet.yml from fleet: used here at once ("config not deployed") and committed to
/// fleet/config-<date>, which reaches main with the next Ship. Creates fleet-state if needed.
#[tauri::command]
pub async fn sync_save_config(app: AppHandle, project_id: String, text: String) -> Result<ProjectCfg, String> {
    if let Err(errs) = fleetyml::parse(&text) {
        return Err(errs.iter().map(|e| format!("line {}: {}", e.line, e.message)).collect::<Vec<_>>().join("\n"));
    }
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let root = PathBuf::from(&p.root_path);
        let c = load_config(&app, &p);
        if !c.has_origin {
            return Err("This project has no `origin` remote. Add the GitHub repo as origin first.".to_string());
        }
        let path = override_path(&p.id);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&path, &text).map_err(|e| e.to_string())?;
        let branch = commit_config_branch(&app, &root, &text, &c)?;
        let _ = std::fs::write(override_branch_path(&p.id), &branch);
        ensure_state_branch(&app, &root);
        refresh_configs(&app, Some(&p.id));
        let _ = app.emit("toast", format!("fleet.yml saved to {branch} — it reaches main with the next Ship."));
        Ok(cfg(&app, &p.id).unwrap_or_default())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn sync_set_paused(app: AppHandle, project_id: Option<String>, paused: bool) -> Result<(), String> {
    match project_id {
        Some(id) => {
            let f = app.state::<Fleet>();
            let mut store = f.store.lock();
            let p = store.projects.iter_mut().find(|p| p.id == id).ok_or("unknown project")?;
            p.sync_paused = paused;
            let _ = app.emit("projects-changed", &store.projects);
            f.mark_dirty();
        }
        None => crate::settings::set_sync_paused_all(paused),
    }
    refresh_configs(&app, None);
    if !paused {
        app.state::<SyncState>().flush_now.store(true, Ordering::Relaxed);
    }
    // Update badges of affected sessions.
    let ids: Vec<(String, String)> = app.state::<Fleet>().store.lock().sessions.iter().filter(|s| s.sync.is_some()).map(|s| (s.id.clone(), s.project_id.clone())).collect();
    for (sid, pid) in ids {
        let paused_now = paused_all() || project(&app, &pid).map(|p| p.sync_paused).unwrap_or(false);
        set_sync(&app, &sid, |i| {
            if paused_now {
                i.badge = "paused".into();
                i.detail = Some("Sync is paused".into());
            }
        });
        // Prompts that finished while paused were never committed: sync them now.
        let was_paused = find(&app, &sid).and_then(|(s, _)| s.sync).is_some_and(|i| i.badge == "paused" && i.detail.as_deref() == Some("Sync is paused"));
        let working = find(&app, &sid).is_some_and(|(s, _)| s.state == SessionState::Working);
        if !paused_now && was_paused && !working {
            let app = app.clone();
            std::thread::spawn(move || sync_session(&app, &sid, false));
        }
    }
    Ok(())
}

/// Fetch every synced project now and retry queued pushes.
#[tauri::command]
pub fn sync_now(app: AppHandle) {
    let st = app.state::<SyncState>();
    st.last_fetch.lock().clear();
    st.fetch_soon.store(true, Ordering::Relaxed);
    st.flush_now.store(true, Ordering::Relaxed);
}

/// Push a session's branch + state now (git panel / badge click).
#[tauri::command]
pub fn sync_session_now(app: AppHandle, session_id: String) {
    std::thread::spawn(move || sync_session(&app, &session_id, false));
}

/// Move a session (claude and all) into a new worktree; its conversation carries on.
fn move_session_to(app: &AppHandle, id: &str, wt: &str) -> Result<(), String> {
    let f = app.state::<Fleet>();
    let (s, project) = find(app, id).ok_or("unknown session")?;
    f.ptys.kill(id);
    let mut transcript = s.transcript_path.clone();
    if let Some(t) = &s.transcript_path {
        let dir = adopt::claude_project_dir(wt);
        let _ = std::fs::create_dir_all(&dir);
        let dest = dir.join(Path::new(t).file_name().unwrap_or_default());
        if std::fs::copy(t, &dest).is_ok() {
            transcript = Some(dest.to_string_lossy().into());
        }
    }
    let mut store = f.store.lock();
    let Some(x) = store.sessions.iter_mut().find(|x| x.id == id) else { return Err("unknown session".into()) };
    x.cwd = wt.to_string();
    x.worktree_path = Some(wt.to_string());
    x.branch = String::new();
    x.transcript_path = transcript;
    x.files_touched.clear();
    x.line_marks.clear();
    x.sync = Some(SyncInfo { badge: "new".into(), detail: Some("Moved into its own synced space".into()), ..Default::default() });
    let snapshot = x.clone();
    let resume = crate::can_resume(&snapshot);
    let res = if snapshot.alive { crate::spawn_claude(app, &f, &snapshot, &project, resume, None, 120, 32) } else { Ok(()) };
    crate::emit_session(app, &snapshot);
    f.mark_dirty();
    drop(store);
    let _ = app.emit("session-respawned", id);
    res
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptResult {
    moved_sessions: usize,
    new_session: Option<String>,
    files: usize,
}

/// Keep the main folder's local work as sessions: files a pre-sync session touched go with
/// it into its own worktree; the rest (and local commits) become a "Local changes" session.
/// The folder then matches GitHub's main again.
#[tauri::command]
pub async fn sync_adopt_local(app: AppHandle, project_id: String, cols: u16, rows: u16) -> Result<AdoptResult, String> {
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let c = cfg(&app, &p.id).filter(|c| c.enabled).ok_or("GitHub sync isn't on for this project.")?;
        let root = PathBuf::from(&p.root_path);
        let git = git_for(&app, &root);
        if let Some(op) = git.operation_in_progress() {
            return Err(format!("A {op} is in progress in the project folder. Finish or abort it first."));
        }
        let presync: Vec<Session> = app.state::<Fleet>().store.lock().sessions.iter().filter(|s| s.project_id == p.id && !s.archived && s.remote.is_none() && s.worktree_path.is_none()).cloned().collect();
        if let Some(busy) = presync.iter().find(|s| s.alive && s.state == SessionState::Working) {
            return Err(format!("{} · {} is working in the project folder. Try again when it finishes.", busy.glyph, busy.title));
        }
        let _ = git.run(&["fetch", "--prune", "origin"]);
        let work = adopt::scan(&git, &c.default_branch);
        if let Some(n) = adopt::stranded_main(&git, &work, &c.default_branch) {
            return Err(format!("Local {} has {n} commit(s) that aren't on GitHub, and the project folder is on another branch. Switch the folder to {} first so fleet can keep them.", c.default_branch, c.default_branch));
        }
        let base = work.head.clone().unwrap_or_else(|| "HEAD".into());
        // Pre-sync sessions start from GitHub's main plus their own files; only "Local changes"
        // keeps the folder's HEAD (its commits that GitHub doesn't have go there, once).
        let origin_main = format!("refs/remotes/origin/{}", c.default_branch);
        let session_base = if git.rev(&origin_main).is_some() { origin_main.clone() } else { base.clone() };
        let rootp = format!("{}/", p.root_path.trim_end_matches('/'));
        let touched: Vec<(String, Vec<String>)> = presync.iter().map(|s| (s.id.clone(), s.files_touched.iter().filter_map(|f| f.strip_prefix(&rootp).map(String::from)).collect())).collect();
        let (owned, rest) = adopt::assign(&work.files, &touched);
        let stash_sha = if work.files.is_empty() { None } else { adopt::stash(&git).map_err(|e| e.to_string())? };
        let mut to_sync = vec![];
        let mut moved = 0;
        let mut new_session = None;
        let moved_out = (|| -> Result<(), String> {
            for s in &presync {
                let wt = crate::worktree::create_detached(&root, &s.title, &session_base)?;
                if let (Some(st), Some(files)) = (&stash_sha, owned.get(&s.id)) {
                    adopt::materialize(&git.at(&wt), st, files).map_err(|e| e.to_string())?;
                    to_sync.push(s.id.clone());
                }
                move_session_to(&app, &s.id, &wt)?;
                moved += 1;
            }
            if !rest.is_empty() || work.ahead > 0 {
                let wt = crate::worktree::create_detached(&root, "local-changes", &base)?;
                if let Some(st) = &stash_sha {
                    adopt::materialize(&git.at(&wt), st, &rest).map_err(|e| e.to_string())?;
                }
                let s = crate::create_session(
                    &app,
                    &p,
                    crate::SessionSpec {
                        title: "Local changes".into(),
                        worktree_path: Some(wt),
                        sync: Some(SyncInfo { badge: "syncing".into(), ..Default::default() }),
                        recap: vec![format!("{} file(s) and {} commit(s) that were in the project folder before sync", rest.len(), work.ahead)],
                        ..Default::default()
                    },
                    cols,
                    rows,
                )?;
                new_session = Some(s.id.clone());
                to_sync.push(s.id);
            }
            Ok(())
        })();
        if let Err(e) = moved_out {
            // Put the folder's work back where it was; nothing stays only in the stash.
            let restored = match &stash_sha {
                Some(st) => git.run(&["stash", "apply", "--index", st]).or_else(|_| git.run(&["stash", "apply", st])).is_ok(),
                None => true,
            };
            if restored {
                if let Some(st) = &stash_sha {
                    adopt::drop_stash(&git, st);
                }
            }
            let where_ = if restored { "Its changes are back in the project folder".to_string() } else { format!("Its changes are saved in git stash {}", stash_sha.clone().unwrap_or_default()) };
            return Err(format!("Moving the project folder's work stopped: {e}. {where_}{}.", if moved > 0 { format!("; {moved} session(s) were already moved (their files are in both places)") } else { String::new() }));
        }
        if let Some(st) = &stash_sha {
            adopt::drop_stash(&git, st);
        }
        adopt::reset_main(&git, &work, &c.default_branch).map_err(|e| e.to_string())?;
        if let Some(conf) = &c.config {
            if let Some(setup) = conf.sync.setup.clone().filter(|s| !s.trim().is_empty()) {
                let wts: Vec<String> = app.state::<Fleet>().store.lock().sessions.iter().filter(|s| s.project_id == p.id && (presync.iter().any(|x| x.id == s.id) || Some(&s.id) == new_session.as_ref())).filter_map(|s| s.worktree_path.clone()).collect();
                for w in wts {
                    run_setup(&app, &w, &setup);
                }
            }
        }
        for id in to_sync {
            let app = app.clone();
            std::thread::spawn(move || sync_session(&app, &id, false));
        }
        refresh_configs(&app, Some(&p.id));
        Ok(AdoptResult { moved_sessions: moved, new_session, files: work.files.len() })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn gh_status() -> gh::GhStatus {
    gh::status()
}

/// A terminal for `brew install gh` or `gh auth login` (shown in the center pane).
#[tauri::command]
pub fn gh_terminal(app: AppHandle, action: String, cols: u16, rows: u16) -> Result<String, String> {
    let line = gh::terminal_command(&action).ok_or("unknown action")?;
    let f = app.state::<Fleet>();
    let id = format!("rterm:gh:{}", f.term_seq.fetch_add(1, Ordering::Relaxed));
    let shell = crate::shell_env::login_env().get("SHELL").cloned().unwrap_or_else(|| "/bin/zsh".into());
    let mut cmd = portable_pty::CommandBuilder::new(&shell);
    cmd.args(["-l", "-c", line]);
    cmd.cwd(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
    cmd.env_clear();
    for (k, v) in crate::shell_env::login_env() {
        cmd.env(k, v);
    }
    crate::spawn_terminal(&app, &id, cmd, cols, rows)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    #[test]
    fn backoff() {
        assert_eq!(super::backoff_ms(0), 5_000);
        assert_eq!(super::backoff_ms(1), 10_000);
        assert_eq!(super::backoff_ms(20), 300_000);
    }
}

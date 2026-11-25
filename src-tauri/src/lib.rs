pub mod git;
pub mod git_cmds;
pub mod fs_cmds;
pub mod lsp;
pub mod settings;
pub mod tray;
pub mod worktree;
pub mod hooks;
pub mod identity;
pub mod model;
pub mod notify;
pub mod persist;
pub mod proc_runtime;
pub mod processes;
pub mod pty;
pub mod shell_env;
pub mod socket;
pub mod ssh;
pub mod sync;
pub mod state_machine;
pub mod titler;
pub mod transcript;
pub mod usage;

use model::{now_ms, ProcessDef, Project, Session, SessionState};
use processes::{ProcessRun, RunStatus};
use std::collections::HashMap;
use parking_lot::Mutex;
use persist::Store;
use portable_pty::CommandBuilder;
use pty::PtyManager;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

pub struct Fleet {
    store: Mutex<Store>,
    ptys: Arc<PtyManager>,
    dirty: AtomicBool,
    /// Sessions currently on screen, and whether the window has focus.
    visible: Mutex<(Vec<String>, bool)>,
    restored: Mutex<usize>,
    hooks_file: PathBuf,
    socket_path: PathBuf,
    hook_bin: Option<PathBuf>,
    runs: Mutex<HashMap<String, ProcessRun>>,
    quit_confirmed: AtomicBool,
    leave_processes_running: AtomicBool,
    git: git_cmds::GitState,
    /// File contents before a session's Edit/Write (session id, path) → text, for line attribution.
    snapshots: Mutex<HashMap<(String, String), String>>,
    term_seq: std::sync::atomic::AtomicU64,
}

type R<T> = Result<T, String>;
fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    projects: Vec<Project>,
    sessions: Vec<Session>,
    ui: serde_json::Value,
    restored: usize,
    warnings: Vec<String>,
    runs: Vec<ProcessRun>,
}

impl Fleet {
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn live_identities(store: &Store, except: &str) -> (Vec<String>, Vec<String>) {
        let live = store.sessions.iter().filter(|s| !s.archived && s.id != except);
        let (mut colors, mut glyphs) = (vec![], vec![]);
        for s in live {
            colors.push(s.color.clone());
            glyphs.push(s.glyph.clone());
        }
        (colors, glyphs)
    }
}

pub(crate) fn emit_session(app: &AppHandle, s: &Session) {
    let _ = app.emit("session-updated", s);
}

fn update_badge(app: &AppHandle, store: &Store) {
    let app2 = app.clone();
    // The tray reads the store; refresh it off this (locked) call path.
    std::thread::spawn(move || tray::refresh(&app2));
    let n = store.sessions.iter().filter(|s| !s.archived && s.state.needs_you()).count();
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_badge_count(if n == 0 { None } else { Some(n as i64) });
    }
}

fn git_branch(cwd: &str) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn claude_bin() -> Result<PathBuf, String> {
    if let Some(p) = settings::claude_path() {
        return shell_env::which(&p).ok_or_else(|| format!("Couldn't find claude at {p} (Settings → Claude)."));
    }
    shell_env::which("claude").ok_or_else(|| "Couldn't find `claude` on your login shell's PATH.".to_string())
}

/// Start (or resume) the claude process for a session.
pub(crate) fn spawn_claude(app: &AppHandle, fleet: &Fleet, s: &Session, project: &Project, resume: bool, first_prompt: Option<&str>, cols: u16, rows: u16) -> R<()> {
    let claude = claude_bin()?;
    let mut cmd = CommandBuilder::new(&claude);
    let resume = resume && uuid::Uuid::parse_str(&s.claude_session_id).is_ok();
    if resume {
        cmd.args(["--resume", &s.claude_session_id]);
    } else {
        cmd.args(["--session-id", &s.claude_session_id]);
    }
    if fleet.hook_bin.is_some() {
        cmd.arg("--settings");
        cmd.arg(&fleet.hooks_file);
    }
    if let Some(m) = s.permission_mode.as_deref().filter(|m| !m.is_empty() && *m != "default") {
        cmd.args(["--permission-mode", m]);
    }
    let yml_args = if s.remote.is_none() { sync::enabled_cfg(app, &project.id).map(|c| c.claude.default_args).unwrap_or_default() } else { vec![] };
    for a in yml_args.into_iter().chain(project.default_claude_args.iter().cloned()).chain(settings::default_args()) {
        cmd.arg(a);
    }
    // Remote session: claude runs here, its file and shell tools run on the server.
    let mut cwd = PathBuf::from(&s.cwd);
    if let Some(conn_id) = &s.remote {
        // From the project we were given: callers hold the store lock, so no find_conn here.
        let c = project.ssh_connections.iter().find(|c| &c.id == conn_id).cloned().ok_or("This session's SSH connection was removed.")?;
        let mcp = sidecar("fleet-remote-mcp").ok_or("fleet-remote-mcp is missing from this build (npm run build:hook).")?;
        let cfg = ssh::write_mcp_config(&c, &mcp)?;
        let md = app.state::<ssh::SshState>().claude_md.lock().get(&c.id).cloned().unwrap_or_default();
        cmd.arg("--mcp-config");
        cmd.arg(&cfg);
        cmd.arg("--disallowedTools");
        for t in ssh::LOCAL_TOOLS_OFF {
            cmd.arg(t);
        }
        // Last flag before the prompt: it takes exactly one value, ending the lists above.
        cmd.arg("--append-system-prompt");
        cmd.arg(ssh::system_prompt(&c, &md));
        cwd = persist::app_dir().join("remote").join(&c.id);
        std::fs::create_dir_all(&cwd).map_err(err)?;
    }
    if let Some(p) = first_prompt.filter(|p| !p.trim().is_empty()) {
        cmd.arg(p);
    }
    cmd.cwd(&cwd);
    cmd.env_clear();
    for (k, v) in shell_env::login_env() {
        cmd.env(k, v);
    }
    cmd.env("FLEET_SESSION_ID", &s.id);
    cmd.env("FLEET_SOCKET", &fleet.socket_path);

    let (app2, id) = (app.clone(), s.id.clone());
    fleet
        .ptys
        .spawn(&s.id, cmd, cols, rows, move |code, current| {
            // Killed (close/archive) or replaced (restart): the new owner manages state.
            if !current {
                return;
            }
            let f = app2.state::<Fleet>();
            let mut st = f.store.lock();
            if let Some(s) = st.sessions.iter_mut().find(|x| x.id == id) {
                if !s.alive {
                    return;
                }
                let before = s.state;
                state_machine::on_exit(s, code, now_ms());
                let s = s.clone();
                emit_session(&app2, &s);
                if s.state == SessionState::Failed && before != SessionState::Failed {
                    maybe_notify(&app2, &f, &st, &s);
                }
                update_badge(&app2, &st);
                f.mark_dirty();
            }
        })
        .map_err(err)?;
    Ok(())
}

fn maybe_notify(app: &AppHandle, fleet: &Fleet, store: &Store, s: &Session) {
    let notify = matches!(s.state, SessionState::NeedsPermission | SessionState::NeedsReply | SessionState::Done | SessionState::Failed);
    if !notify || s.archived || settings::muted(&s.project_id, &s.id) {
        return;
    }
    {
        let v = fleet.visible.lock();
        if v.1 && v.0.contains(&s.id) {
            return;
        }
    }
    let project = store.projects.iter().find(|p| p.id == s.project_id).map(|p| p.name.clone()).unwrap_or_default();
    let what = match s.state {
        SessionState::Done => "done".to_string(),
        SessionState::Failed => s.pending_question.clone().unwrap_or_else(|| "failed".into()),
        _ => s.pending_question.clone().unwrap_or_else(|| "needs you".into()),
    };
    let sound = settings::sounds().then(|| match s.state {
        SessionState::Done => "Hero",
        SessionState::Failed => "Basso",
        _ => "Glass",
    });
    notify::send_with_sound(&s.id, &format!("fleet — {project}"), &format!("{} {} · {}: {}", s.state.symbol(), s.glyph, s.title, what), sound);
    let _ = app;
}

fn handle_hook(app: &AppHandle, env: hooks::Envelope) {
    let fleet = app.state::<Fleet>();
    let parsed = hooks::parse(&env.payload);
    attribute_lines(app, &env.fleet_session_id, &parsed.event);
    let mut store = fleet.store.lock();
    let Some(idx) = store.sessions.iter().position(|s| s.id == env.fleet_session_id) else { return };
    let fx = state_machine::apply(&mut store.sessions[idx], &parsed, now_ms());
    let s = store.sessions[idx].clone();
    if fx.title_changed && !s.pinned_glyph {
        let (_, glyphs) = Fleet::live_identities(&store, &s.id);
        let refs: Vec<&str> = glyphs.iter().map(|g| g.as_str()).collect();
        store.sessions[idx].glyph = identity::pick_glyph(&s.title, &refs);
    }
    let s = store.sessions[idx].clone();
    if let Some(p) = store.projects.iter_mut().find(|p| p.id == s.project_id) {
        p.last_activity = now_ms();
    }
    if fx.changed {
        emit_session(app, &s);
        fleet.mark_dirty();
    }
    if fx.state_changed {
        maybe_notify(app, &fleet, &store, &s);
        update_badge(app, &store);
    }
    drop(store);

    if let Some((ms, wait)) = fx.turn_done {
        usage::append([usage::Event { turn_ms: Some(ms), wait_ms: Some(wait), ..Default::default() }], &s.project_id, &s.id);
    }

    let is_stop = matches!(parsed.event, hooks::HookEvent::Stop { .. });
    if is_stop {
        if fx.regenerate_title && !s.title_locked && settings::auto_titles() {
            app.state::<sync::SyncState>().titling.lock().insert(s.id.clone());
        }
        sync::on_stop(app, &s.id);
    }
    if fx.refresh_context || fx.regenerate_title {
        let app = app.clone();
        let (id, transcript, cwd) = (s.id.clone(), s.transcript_path.clone(), s.cwd.clone());
        let regen = fx.regenerate_title && !s.title_locked && settings::auto_titles();
        let last_prompt = s.last_prompt.clone().unwrap_or_default();
        let last_message = match &parsed.event {
            hooks::HookEvent::Stop { last_message } => last_message.clone(),
            _ => String::new(),
        };
        let project_id = s.project_id.clone();
        std::thread::spawn(move || {
            if let Some(t) = transcript.as_deref() {
                refresh_usage(&app, &id, &project_id, Path::new(t));
            }
            let pct = transcript.as_deref().and_then(transcript::context_pct);
            let branch = if regen { Some(git_branch(&cwd)) } else { None };
            update_session(&app, &id, |s| {
                let mut ch = false;
                if pct.is_some() && s.context_pct != pct {
                    s.context_pct = pct;
                    ch = true;
                }
                if let Some(b) = &branch {
                    if !b.is_empty() && &s.branch != b {
                        s.branch = b.clone();
                        ch = true;
                    }
                }
                ch
            });
            if regen && !last_message.trim().is_empty() {
                let Ok(claude) = claude_bin() else {
                    // The Stop's sync waits on this entry: never leave it behind.
                    app.state::<sync::SyncState>().titling.lock().remove(&id);
                    return;
                };
                let dir = persist::app_dir().join("titler");
                let _ = std::fs::create_dir_all(&dir);
                match titler::generate(&claude, &settings::title_model(), &dir, &last_prompt, &last_message) {
                    Ok(tr) => {
                        update_session(&app, &id, |s| {
                            if s.title_locked {
                                return false;
                            }
                            if tr.title != s.title {
                                let old = std::mem::replace(&mut s.title, tr.title.clone());
                                if !state_machine::is_placeholder_title(&old) {
                                    s.title_history.push(old);
                                }
                            }
                            s.recap = tr.recap.clone();
                            true
                        });
                    }
                    Err(e) => eprintln!("[fleet] title generation failed: {e}"),
                }
            }
            app.state::<sync::SyncState>().titling.lock().remove(&id);
        });
    }
}

/// Snapshot a file before a session edits it; afterwards record which lines changed.
fn attribute_lines(app: &AppHandle, session_id: &str, ev: &hooks::HookEvent) {
    const MAX: u64 = 2 * 1024 * 1024;
    let fleet = app.state::<Fleet>();
    match ev {
        hooks::HookEvent::PreToolUse { file: Some(f), .. } => {
            let small = std::fs::metadata(f).map(|m| m.len() <= MAX).unwrap_or(true);
            if small {
                let text = std::fs::read_to_string(f).unwrap_or_default();
                fleet.snapshots.lock().insert((session_id.to_string(), f.clone()), text);
            }
        }
        hooks::HookEvent::PostToolUse { file: Some(f), .. } => {
            let Some(before) = fleet.snapshots.lock().remove(&(session_id.to_string(), f.clone())) else { return };
            let Ok(after) = std::fs::read_to_string(f) else { return };
            let ranges = fs_cmds::changed_ranges(&before, &after);
            if ranges.is_empty() {
                return;
            }
            update_session(app, session_id, |s| {
                let marks = s.line_marks.entry(f.clone()).or_default();
                marks.extend(ranges.iter().copied());
                marks.sort();
                marks.dedup();
                if marks.len() > 400 {
                    let n = marks.len() - 400;
                    marks.drain(..n);
                }
                true
            });
        }
        _ => {}
    }
}

/// Read new transcript lines into the session's token/cost totals and log them per day.
fn refresh_usage(app: &AppHandle, id: &str, project_id: &str, transcript: &Path) {
    let _scan = usage::SCAN_LOCK.lock();
    let Some(mut m) = app.state::<Fleet>().store.lock().sessions.iter().find(|s| s.id == id).map(|s| s.metrics.clone()) else { return };
    let days = usage::scan(&mut m, transcript);
    // Cursors move even when no usage line was added.
    update_session(app, id, |s| {
        let ch = s.metrics.main != m.main || s.metrics.sub != m.sub;
        s.metrics.main = m.main;
        s.metrics.sub = m.sub;
        s.metrics.cursors = m.cursors;
        ch
    });
    usage::append(days.into_values(), project_id, id);
}

pub(crate) fn update_session(app: &AppHandle, id: &str, f: impl FnOnce(&mut Session) -> bool) {
    let fleet = app.state::<Fleet>();
    let mut store = fleet.store.lock();
    if let Some(s) = store.sessions.iter_mut().find(|s| s.id == id) {
        if f(s) {
            let s = s.clone();
            emit_session(app, &s);
            fleet.mark_dirty();
        }
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
fn usage_report(days: u32) -> Vec<usage::DayRow> {
    usage::report(days, now_ms())
}

#[tauri::command]
fn get_snapshot(fleet: State<Fleet>) -> Snapshot {
    let store = fleet.store.lock();
    let mut warnings = vec![];
    if fleet.hook_bin.is_none() {
        warnings.push("fleet-hook binary not found — session state tracking is off. Run `npm run build:hook`.".into());
    }
    if shell_env::which("claude").is_none() {
        warnings.push("`claude` was not found on your login shell's PATH.".into());
    }
    Snapshot {
        projects: store.projects.clone(),
        sessions: store.sessions.clone(),
        ui: persist::load_ui(),
        restored: std::mem::take(&mut *fleet.restored.lock()),
        warnings,
        runs: fleet.runs.lock().values().cloned().collect(),
    }
}

#[tauri::command]
fn add_project(path: String, fleet: State<Fleet>, app: AppHandle) -> R<Project> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("{path} is not a folder"));
    }
    let mut store = fleet.store.lock();
    if let Some(p) = store.projects.iter().find(|p| p.root_path == path) {
        return Ok(p.clone());
    }
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path.clone());
    let color_family = identity::PALETTE[store.projects.len() % identity::PALETTE.len()].to_string();
    let processes = processes::load_project_file(&root).map(|f| f.processes).unwrap_or_default();
    let p = Project {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        root_path: path,
        color_family,
        processes,
        default_claude_args: vec![],
        collapsed: false,
        last_activity: now_ms(),
        processes_offered: false,
        ssh_connections: vec![],
        sync_paused: false,
    };
    store.projects.push(p.clone());
    let _ = app.emit("projects-changed", &store.projects);
    fleet.mark_dirty();
    drop(store);
    proc_runtime::autostart(&app, Some(&[p.id.clone()]));
    Ok(p)
}

#[tauri::command]
fn remove_project(id: String, fleet: State<Fleet>, app: AppHandle) -> R<()> {
    let keys: Vec<String> = fleet.runs.lock().values().filter(|r| r.project_id == id).map(|r| r.key.clone()).collect();
    for k in keys {
        proc_runtime::stop(&app, &k, false);
    }
    let mut store = fleet.store.lock();
    let ids: Vec<String> = store.sessions.iter().filter(|s| s.project_id == id).map(|s| s.id.clone()).collect();
    for sid in &ids {
        fleet.ptys.kill(sid);
        let _ = app.emit("session-removed", sid);
    }
    store.sessions.retain(|s| s.project_id != id);
    store.projects.retain(|p| p.id != id);
    let _ = app.emit("projects-changed", &store.projects);
    update_badge(&app, &store);
    fleet.mark_dirty();
    Ok(())
}

#[tauri::command]
fn set_project_collapsed(id: String, collapsed: bool, fleet: State<Fleet>, app: AppHandle) {
    let mut store = fleet.store.lock();
    if let Some(p) = store.projects.iter_mut().find(|p| p.id == id) {
        p.collapsed = collapsed;
    }
    let _ = app.emit("projects-changed", &store.projects);
    fleet.mark_dirty();
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn new_session(project_id: String, prompt: Option<String>, permission_mode: Option<String>, worktree: Option<bool>, remote: Option<String>, cols: u16, rows: u16, fleet: State<Fleet>, app: AppHandle) -> R<Session> {
    let project = fleet.store.lock().projects.iter().find(|p| p.id == project_id).cloned().ok_or("unknown project")?;
    if let Some(conn) = &remote {
        if !project.ssh_connections.iter().any(|c| &c.id == conn) {
            return Err("unknown SSH connection".into());
        }
        if !ssh::is_connected(&app, conn) {
            return Err("Connect to the server first.".into());
        }
    }
    let worktree = if remote.is_some() { None } else { worktree };
    let title = prompt.as_deref().filter(|p| !p.trim().is_empty()).map(identity::title_from_prompt).unwrap_or_else(|| "New session".into());
    // Worktree first (slow git call) without holding the store lock. Synced projects always
    // give a session its own worktree (detached at origin's main until its first prompt ends).
    let synced = remote.is_none() && sync::enabled_cfg(&app, &project.id).is_some();
    let synced_wt = if synced { sync::session_worktree(&app, &project, &title)? } else { None };
    let worktree_path = if synced_wt.is_some() {
        synced_wt
    } else if worktree == Some(true) {
        Some(worktree::create(Path::new(&project.root_path), if title == "New session" { "session" } else { &title })?)
    } else {
        None
    };
    let branch = if remote.is_some() { String::new() } else { git_branch(worktree_path.as_deref().unwrap_or(&project.root_path)) };
    create_session(
        &app,
        &project,
        SessionSpec {
            title,
            prompt,
            permission_mode,
            worktree_path,
            remote,
            branch: if synced { String::new() } else { branch },
            sync: synced.then(|| model::SyncInfo { badge: "new".into(), detail: Some("Gets its branch when the first prompt finishes".into()), ..Default::default() }),
            ..Default::default()
        },
        cols,
        rows,
    )
}

/// Everything a new session record needs; identity is picked unless given.
#[derive(Default)]
pub(crate) struct SessionSpec {
    pub title: String,
    pub prompt: Option<String>,
    pub permission_mode: Option<String>,
    pub worktree_path: Option<String>,
    pub remote: Option<String>,
    pub branch: String,
    pub glyph: Option<String>,
    pub color: Option<String>,
    pub title_history: Vec<String>,
    pub recap: Vec<String>,
    pub prompt_count: u32,
    pub sync: Option<model::SyncInfo>,
    /// Resume this claude session (synced chat) instead of starting a new one.
    pub claude_session_id: Option<String>,
    pub transcript_path: Option<String>,
}

/// Create the session record, start claude in it and announce it.
pub(crate) fn create_session(app: &AppHandle, project: &Project, spec: SessionSpec, cols: u16, rows: u16) -> R<Session> {
    let fleet = app.state::<Fleet>();
    let permission_mode = spec.permission_mode.or_else(settings::default_permission_mode);
    let mut store = fleet.store.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let (colors, glyphs) = Fleet::live_identities(&store, &id);
    let crefs: Vec<&str> = colors.iter().map(|c| c.as_str()).collect();
    let grefs: Vec<&str> = glyphs.iter().map(|c| c.as_str()).collect();
    let now = now_ms();
    let title = spec.title;
    let prompt = spec.prompt;
    let s = Session {
        id: id.clone(),
        project_id: project.id.clone(),
        claude_session_id: spec.claude_session_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        pinned_glyph: spec.glyph.is_some(),
        pinned_color: spec.color.is_some(),
        glyph: spec.glyph.unwrap_or_else(|| identity::pick_glyph(if title == "New session" { &project.name } else { &title }, &grefs)),
        color: spec.color.unwrap_or_else(|| identity::pick_color_from(&settings::palette(), &crefs)),
        title,
        title_history: spec.title_history,
        recap: spec.recap,
        cwd: match &spec.remote {
            Some(c) => persist::app_dir().join("remote").join(c).to_string_lossy().into(),
            None => spec.worktree_path.clone().unwrap_or_else(|| project.root_path.clone()),
        },
        branch: spec.branch,
        worktree_path: spec.worktree_path,
        state: if prompt.as_deref().map(|p| !p.trim().is_empty()).unwrap_or(false) { SessionState::Working } else { SessionState::Idle },
        state_since: now,
        pending_question: None,
        context_pct: None,
        files_touched: vec![],
        created_at: now,
        pinned: false,
        title_locked: false,
        archived: false,
        permission_mode,
        alive: true,
        transcript_path: spec.transcript_path.clone(),
        last_prompt: prompt.clone(),
        model: None,
        exit_code: None,
        line_marks: Default::default(),
        remote: spec.remote,
        prompt_count: spec.prompt_count,
        sync: spec.sync,
        metrics: Default::default(),
    };
    let resume = spec.claude_session_id.is_some() && can_resume(&s);
    spawn_claude(app, &fleet, &s, project, resume, prompt.as_deref(), cols, rows)?;
    store.sessions.push(s.clone());
    if let Some(p) = store.projects.iter_mut().find(|p| p.id == project.id) {
        p.last_activity = now;
    }
    let _ = app.emit("projects-changed", &store.projects);
    emit_session(app, &s);
    fleet.mark_dirty();
    Ok(s)
}

#[tauri::command]
fn pty_attach(id: String, on_data: Channel<InvokeResponseBody>, fleet: State<Fleet>) -> R<bool> {
    if !fleet.ptys.exists(&id) {
        return Ok(false);
    }
    fleet.ptys.attach(&id, Arc::new(on_data)).map_err(err)?;
    Ok(true)
}

#[tauri::command]
fn pty_write(id: String, data: String, fleet: State<Fleet>) -> R<()> {
    fleet.ptys.write(&id, data.as_bytes()).map_err(err)
}

#[tauri::command]
fn pty_resize(id: String, cols: u16, rows: u16, fleet: State<Fleet>) -> R<()> {
    fleet.ptys.resize(&id, cols, rows).map_err(err)
}

/// Restart (or resume an archived/dead session) with `claude --resume`.
#[tauri::command]
fn restart_session(id: String, cols: u16, rows: u16, app: AppHandle) -> R<Session> {
    resume_session(&app, &id, None, cols, rows)
}

/// Start a dead/archived session again (`claude --resume` when it has a transcript), optionally
/// submitting `prompt` as its first message.
pub(crate) fn resume_session(app: &AppHandle, id: &str, prompt: Option<&str>, cols: u16, rows: u16) -> R<Session> {
    let fleet = app.state::<Fleet>();
    let mut store = fleet.store.lock();
    let idx = store.sessions.iter().position(|s| s.id == id).ok_or("unknown session")?;
    let project = store.projects.iter().find(|p| p.id == store.sessions[idx].project_id).cloned().ok_or("unknown project")?;
    if uuid::Uuid::parse_str(&store.sessions[idx].claude_session_id).is_err() {
        store.sessions[idx].claude_session_id = uuid::Uuid::new_v4().to_string();
    }
    let resume = can_resume(&store.sessions[idx]);
    {
        let s = &mut store.sessions[idx];
        s.archived = false;
        s.alive = true;
        s.exit_code = None;
        s.state = if prompt.is_some() { SessionState::Working } else { SessionState::Idle };
        s.state_since = now_ms();
        s.pending_question = None;
        if let Some(p) = prompt {
            s.last_prompt = Some(p.to_string());
        }
    }
    let s = store.sessions[idx].clone();
    spawn_claude(app, &fleet, &s, &project, resume, prompt, cols, rows)?;
    emit_session(app, &s);
    update_badge(app, &store);
    fleet.mark_dirty();
    Ok(s)
}

pub(crate) fn can_resume(s: &Session) -> bool {
    if s.transcript_path.as_deref().map(|p| Path::new(p).exists()).unwrap_or(false) {
        return true;
    }
    // No transcript path reported by hooks (e.g. claude ran but no hook delivered it): claude
    // still refuses `--session-id` for an id that already has a transcript ("already in use"),
    // so look for it where claude keeps transcripts. Scan every project dir because the
    // process cwd isn't always `s.cwd` (worktrees, remote sessions).
    let file = format!("{}.jsonl", s.claude_session_id);
    if sync::adopt::claude_project_dir(&s.cwd).join(&file).exists() {
        return true;
    }
    let root = dirs::home_dir().unwrap_or_default().join(".claude").join("projects");
    std::fs::read_dir(root)
        .map(|rd| rd.flatten().any(|e| e.path().join(&file).exists()))
        .unwrap_or(false)
}

#[tauri::command]
fn close_session(id: String, archive: bool, fleet: State<Fleet>, app: AppHandle) -> R<()> {
    let mut store = fleet.store.lock();
    if let Some(s) = store.sessions.iter_mut().find(|s| s.id == id) {
        s.alive = false;
    }
    if let Some(s) = store.sessions.iter().find(|s| s.id == id) {
        sync::on_close(&app, s);
    }
    fleet.ptys.kill(&id);
    let prefix = format!("console:{id}:");
    fleet.ptys.kill_where(|p| p.starts_with(&prefix));
    if archive {
        if let Some(s) = store.sessions.iter_mut().find(|s| s.id == id) {
            s.archived = true;
            s.state = SessionState::Idle;
            s.state_since = now_ms();
            s.pending_question = None;
            let s = s.clone();
            emit_session(&app, &s);
        }
    } else {
        store.sessions.retain(|s| s.id != id);
        let _ = app.emit("session-removed", &id);
    }
    update_badge(&app, &store);
    fleet.mark_dirty();
    Ok(())
}

#[tauri::command]
fn rename_session(id: String, title: String, fleet: State<Fleet>, app: AppHandle) -> R<()> {
    let mut store = fleet.store.lock();
    let (_, glyphs) = Fleet::live_identities(&store, &id);
    let Some(s) = store.sessions.iter_mut().find(|s| s.id == id) else { return Err("unknown session".into()) };
    let t = title.trim();
    if t.is_empty() {
        s.title_locked = false; // reset → auto titles resume
    } else {
        if t == s.title {
            return Ok(()); // nothing renamed; don't freeze auto titles
        }
        let old = std::mem::replace(&mut s.title, t.to_string());
        s.title_history.push(old);
        s.title_locked = true;
        if !s.pinned_glyph {
            let refs: Vec<&str> = glyphs.iter().map(|g| g.as_str()).collect();
            s.glyph = identity::pick_glyph(t, &refs);
        }
    }
    let s = s.clone();
    emit_session(&app, &s);
    fleet.mark_dirty();
    Ok(())
}

#[tauri::command]
fn pin_identity(id: String, color: Option<String>, glyph: Option<String>, fleet: State<Fleet>, app: AppHandle) -> R<()> {
    let mut store = fleet.store.lock();
    let Some(s) = store.sessions.iter_mut().find(|s| s.id == id) else { return Err("unknown session".into()) };
    if let Some(c) = color {
        s.color = c;
        s.pinned_color = true;
    }
    if let Some(g) = glyph {
        let g: String = g.to_uppercase().chars().filter(|c| c.is_ascii_alphanumeric()).take(2).collect();
        if g.len() == 2 {
            s.glyph = g;
            s.pinned_glyph = true;
        }
    }
    let s = s.clone();
    emit_session(&app, &s);
    fleet.mark_dirty();
    Ok(())
}

#[tauri::command]
fn set_visible(ids: Vec<String>, focused: bool, fleet: State<Fleet>) {
    *fleet.visible.lock() = (ids, focused);
}

#[tauri::command]
fn save_ui(ui: serde_json::Value) {
    persist::save_ui(&ui);
}

// ---------------------------------------------------------------- processes

fn project_root(fleet: &Fleet, project_id: &str) -> R<PathBuf> {
    fleet.store.lock().projects.iter().find(|p| p.id == project_id).map(|p| PathBuf::from(&p.root_path)).ok_or_else(|| "unknown project".into())
}

/// Suggested processes (package.json scripts, Procfile, compose, Makefile) not yet defined.
#[tauri::command]
fn detect_processes(project_id: String, fleet: State<Fleet>) -> R<Vec<ProcessDef>> {
    let root = project_root(&fleet, &project_id)?;
    let existing: Vec<String> = fleet.store.lock().projects.iter().find(|p| p.id == project_id).map(|p| p.processes.iter().map(|d| d.command.clone()).collect()).unwrap_or_default();
    Ok(processes::detect(&root).into_iter().filter(|d| !existing.contains(&d.command)).collect())
}

#[tauri::command]
fn set_processes_offered(project_id: String, fleet: State<Fleet>, app: AppHandle) {
    let mut store = fleet.store.lock();
    if let Some(p) = store.projects.iter_mut().find(|p| p.id == project_id) {
        p.processes_offered = true;
    }
    let _ = app.emit("projects-changed", &store.projects);
    fleet.mark_dirty();
}

/// Create or update a process definition; saved to <root>/.fleet/project.json.
#[tauri::command]
fn save_process(project_id: String, mut def: ProcessDef, keep_local: Option<bool>, fleet: State<Fleet>, app: AppHandle) -> R<ProcessDef> {
    if def.command.trim().is_empty() {
        return Err("Command is required".into());
    }
    if def.id.is_empty() {
        def.id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    }
    if def.name.trim().is_empty() {
        def.name = def.command.clone();
    }
    if def.cwd.trim().is_empty() {
        def.cwd = ".".into();
    }
    let mut store = fleet.store.lock();
    let p = store.projects.iter_mut().find(|p| p.id == project_id).ok_or("unknown project")?;
    match p.processes.iter_mut().find(|d| d.id == def.id) {
        Some(d) => *d = def.clone(),
        None => p.processes.push(def.clone()),
    }
    p.processes_offered = true;
    let root = PathBuf::from(&p.root_path);
    processes::save_project_file(&root, &p.processes).map_err(err)?;
    if keep_local == Some(true) {
        processes::exclude_locally(&root, ".fleet/project.json").map_err(err)?;
    }
    let _ = app.emit("projects-changed", &store.projects);
    fleet.mark_dirty();
    Ok(def)
}

#[tauri::command]
fn delete_process(project_id: String, def_id: String, fleet: State<Fleet>, app: AppHandle) -> R<()> {
    let key = processes::key(&project_id, &def_id);
    proc_runtime::stop(&app, &key, false);
    let mut store = fleet.store.lock();
    let p = store.projects.iter_mut().find(|p| p.id == project_id).ok_or("unknown project")?;
    p.processes.retain(|d| d.id != def_id);
    processes::save_project_file(&PathBuf::from(&p.root_path), &p.processes).map_err(err)?;
    let _ = app.emit("projects-changed", &store.projects);
    fleet.mark_dirty();
    drop(store);
    fleet.runs.lock().remove(&key);
    let _ = app.emit("process-removed", &key);
    Ok(())
}

#[tauri::command]
fn process_start(project_id: String, def_id: String, cols: u16, rows: u16, session_id: Option<String>, app: AppHandle) -> R<ProcessRun> {
    match session_id {
        Some(s) => proc_runtime::start_in(&app, &project_id, &def_id, cols, rows, Some(s).filter(|s| !s.is_empty())),
        None => proc_runtime::start(&app, &project_id, &def_id, cols, rows),
    }
}

#[tauri::command]
fn process_stop(project_id: String, def_id: String, app: AppHandle) {
    proc_runtime::stop(&app, &processes::key(&project_id, &def_id), false);
}

#[tauri::command]
fn process_restart(project_id: String, def_id: String, cols: u16, rows: u16, session_id: Option<String>, app: AppHandle) -> R<()> {
    proc_runtime::restart(&app, &project_id, &def_id, cols, rows, session_id)
}

#[tauri::command]
fn pty_clear(id: String, fleet: State<Fleet>) {
    fleet.ptys.clear_replay(&id);
}

/// Paste text into a session's Claude prompt (bracketed paste, not submitted).
#[tauri::command]
fn send_to_session(session_id: String, text: String, fleet: State<Fleet>) -> R<()> {
    let clean = text.replace("\x1b[201~", "");
    let data = format!("\x1b[200~{clean}\x1b[201~");
    fleet.ptys.write(&session_id, data.as_bytes()).map_err(err)
}

/// Answer to the quit prompt shown while processes are running.
#[tauri::command]
fn quit_app(stop_processes: bool, fleet: State<Fleet>, app: AppHandle) {
    fleet.quit_confirmed.store(true, Ordering::SeqCst);
    if !stop_processes {
        fleet.leave_processes_running.store(true, Ordering::SeqCst);
        let detached: Vec<ProcessRun> = fleet.runs.lock().values().filter(|r| r.status == RunStatus::Running && r.pid.is_some()).cloned().collect();
        fleet.store.lock().detached = detached;
        app.exit(0);
        return;
    }
    fleet.store.lock().detached.clear();
    let keys: Vec<String> = fleet.runs.lock().values().filter(|r| r.status == RunStatus::Running).map(|r| r.key.clone()).collect();
    for k in &keys {
        proc_runtime::stop(&app, k, false);
    }
    // Give them up to 5 s to exit cleanly, then make sure nothing keeps a port busy.
    std::thread::spawn(move || {
        for _ in 0..50 {
            if proc_runtime::running_count(&app.state::<Fleet>()) == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let pids: Vec<u32> = app.state::<Fleet>().runs.lock().values().filter(|r| r.status == RunStatus::Running).filter_map(|r| r.pid).collect();
        for pid in pids {
            pty::signal_group(pid, libc::SIGKILL);
        }
        app.exit(0);
    });
}

fn should_confirm_quit(fleet: &Fleet) -> bool {
    !fleet.quit_confirmed.load(Ordering::SeqCst) && proc_runtime::running_count(fleet) > 0
}

// ---------------------------------------------------------------- consoles & remote terminals

pub(crate) fn spawn_terminal(app: &AppHandle, id: &str, cmd: CommandBuilder, cols: u16, rows: u16) -> R<()> {
    let (app2, id2) = (app.clone(), id.to_string());
    app.state::<Fleet>()
        .ptys
        .spawn(id, cmd, cols, rows, move |code, current| {
            if current {
                let _ = app2.emit("terminal-exit", serde_json::json!({ "id": id2, "code": code }));
            }
        })
        .map_err(err)?;
    Ok(())
}

fn local_shell(cwd: &str) -> CommandBuilder {
    let shell = shell_env::login_env().get("SHELL").cloned().unwrap_or_else(|| "/bin/zsh".into());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.arg("-l");
    cmd.cwd(cwd);
    cmd.env_clear();
    for (k, v) in shell_env::login_env() {
        cmd.env(k, v);
    }
    cmd
}

/// ⌥N: a shell at the bottom of a session — in its folder, or on its server for SSH sessions.
#[tauri::command]
fn console_open(session_id: String, cols: u16, rows: u16, fleet: State<Fleet>, app: AppHandle) -> R<String> {
    let s = fleet.store.lock().sessions.iter().find(|s| s.id == session_id).cloned().ok_or("unknown session")?;
    let n = fleet.term_seq.fetch_add(1, Ordering::Relaxed);
    let id = format!("console:{session_id}:{n}");
    let cmd = match &s.remote {
        Some(conn) => {
            let (_, c) = ssh::find_conn(&app, conn).ok_or("This session's SSH connection was removed.")?;
            if !ssh::is_connected(&app, conn) {
                return Err(format!("Connect to {} first.", c.name));
            }
            ssh::shell_command(&c)
        }
        None => {
            let project_root = fleet.store.lock().projects.iter().find(|p| p.id == s.project_id).map(|p| p.root_path.clone()).unwrap_or_default();
            local_shell(s.worktree_path.as_deref().unwrap_or(if project_root.is_empty() { &s.cwd } else { &project_root }))
        }
    };
    spawn_terminal(&app, &id, cmd, cols, rows)?;
    Ok(id)
}

#[tauri::command]
fn terminal_close(id: String, fleet: State<Fleet>) {
    fleet.ptys.kill(&id);
}

/// A standalone shell on a connection's server, in its folder.
#[tauri::command]
fn remote_terminal_open(conn_id: String, cols: u16, rows: u16, fleet: State<Fleet>, app: AppHandle) -> R<String> {
    let (_, c) = ssh::find_conn(&app, &conn_id).ok_or("unknown connection")?;
    if !ssh::is_connected(&app, &conn_id) {
        return Err(format!("Connect to {} first.", c.name));
    }
    let id = format!("rterm:{conn_id}:{}", fleet.term_seq.fetch_add(1, Ordering::Relaxed));
    spawn_terminal(&app, &id, ssh::shell_command(&c), cols, rows)?;
    Ok(id)
}

// ---------------------------------------------------------------- window appearance (themes)

/// Native side of a theme: macOS vibrancy (Glass), light/dark chrome, window background.
fn apply_window_appearance(app: &AppHandle, vibrancy: bool, dark: bool, background: &str) {
    use tauri::window::{Color, Effect, EffectState, EffectsBuilder};
    let Some(w) = app.get_webview_window("main") else { return };
    let _ = w.set_theme(Some(if dark { tauri::Theme::Dark } else { tauri::Theme::Light }));
    if vibrancy {
        let _ = w.set_effects(EffectsBuilder::new().effect(Effect::UnderWindowBackground).state(EffectState::Active).build());
    } else {
        let _ = w.set_effects(None);
    }
    let hex = background.trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    if hex.len() >= 6 {
        let a = if hex.len() >= 8 { byte(6) } else { 255 };
        let _ = w.set_background_color(Some(Color(byte(0), byte(2), byte(4), a)));
    }
}

/// (vibrancy, dark, background) for a theme id, so the window is right before the UI loads.
fn theme_window(id: &str) -> (bool, bool, &'static str) {
    match id {
        "glass" => (true, true, "#00000000"),
        "light" => (false, false, "#FFFFFF"),
        "pear" => (false, false, "#FFFFFF"),
        "soft" => (false, false, "#E4E9F0"),
        "hacker" => (false, true, "#050805"),
        "cyberpunk" => (false, true, "#0A0B14"),
        "retro" => (false, true, "#120D29"),
        _ => (false, true, "#0D1117"),
    }
}

#[tauri::command]
fn set_window_appearance(vibrancy: bool, dark: bool, background: String, app: AppHandle) {
    apply_window_appearance(&app, vibrancy, dark, &background);
}

// ---------------------------------------------------------------- setup

fn hook_binary() -> Option<PathBuf> {
    sidecar("fleet-hook")
}

/// A bundled helper binary: next to the executable in the .app, else the dev build.
fn sidecar(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    [
        dir.join(name),
        manifest.join(format!("../{name}/target/release/{name}")),
        manifest.join(format!("../{name}/target/debug/{name}")),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .and_then(|p| p.canonicalize().ok())
}

fn restore_sessions(app: &AppHandle) {
    let fleet = app.state::<Fleet>();
    let mut store = fleet.store.lock();
    let mut n = 0;
    let projects = store.projects.clone();
    for i in 0..store.sessions.len() {
        let s = &mut store.sessions[i];
        s.alive = false;
        if s.archived {
            continue;
        }
        let Some(project) = projects.iter().find(|p| p.id == s.project_id) else { continue };
        s.state = SessionState::Idle;
        s.state_since = now_ms();
        s.pending_question = None;
        s.alive = true;
        if uuid::Uuid::parse_str(&s.claude_session_id).is_err() {
            s.claude_session_id = uuid::Uuid::new_v4().to_string();
        }
        let resume = can_resume(s);
        let snapshot = s.clone();
        match spawn_claude(app, &fleet, &snapshot, project, resume, None, 120, 32) {
            Ok(()) => n += 1,
            Err(e) => {
                let s = &mut store.sessions[i];
                s.alive = false;
                s.state = SessionState::Failed;
                s.pending_question = Some(e);
            }
        }
    }
    *fleet.restored.lock() = n;
}

pub fn run() {
    let app_dir = persist::app_dir();
    let mut socket_path = app_dir.join("fleet.sock");
    if socket_path.as_os_str().len() > 100 {
        // Unix socket paths are limited to 104 bytes: use a short one for long app folders.
        let h = app_dir.to_string_lossy().bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
        socket_path = std::env::temp_dir().join(format!("fleet-{:08x}.sock", h as u32));
    }
    let hooks_file = app_dir.join("fleet-hooks.json");
    let hook_bin = hook_binary();
    if let Some(bin) = &hook_bin {
        let _ = std::fs::write(&hooks_file, serde_json::to_vec_pretty(&hooks::settings_json(&bin.to_string_lossy())).unwrap());
    } else {
        eprintln!("[fleet] fleet-hook binary not found; state tracking disabled");
    }
    // Warm the login-shell env off the main thread.
    std::thread::spawn(|| {
        shell_env::login_env();
    });
    notify::init();

    let fleet = Fleet {
        store: Mutex::new(persist::load_store()),
        ptys: PtyManager::new(),
        dirty: AtomicBool::new(false),
        visible: Mutex::new((vec![], true)),
        restored: Mutex::new(0),
        hooks_file,
        socket_path: socket_path.clone(),
        hook_bin,
        runs: Mutex::new(HashMap::new()),
        quit_confirmed: AtomicBool::new(false),
        leave_processes_running: AtomicBool::new(false),
        git: git_cmds::GitState::default(),
        snapshots: Mutex::new(HashMap::new()),
        term_seq: std::sync::atomic::AtomicU64::new(1),
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(fleet)
        .manage(lsp::LspState::default())
        .manage(ssh::SshState::default())
        .manage(sync::SyncState::default())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            usage_report,
            add_project,
            remove_project,
            set_project_collapsed,
            new_session,
            pty_attach,
            pty_write,
            pty_resize,
            restart_session,
            close_session,
            rename_session,
            pin_identity,
            set_visible,
            save_ui,
            detect_processes,
            set_processes_offered,
            save_process,
            delete_process,
            process_start,
            process_stop,
            process_restart,
            pty_clear,
            send_to_session,
            quit_app,
            git_cmds::git_status,
            git_cmds::git_diff,
            git_cmds::git_hunk,
            git_cmds::git_stage,
            git_cmds::git_unstage,
            git_cmds::git_discard,
            git_cmds::git_commit,
            git_cmds::git_remote,
            git_cmds::git_cancel,
            git_cmds::git_run,
            git_cmds::git_log,
            git_cmds::git_commit_files,
            git_cmds::git_commit_diff,
            git_cmds::git_branches,
            git_cmds::git_stashes,
            git_cmds::git_write_message,
            git_cmds::git_watch,
            fs_cmds::fs_list,
            fs_cmds::fs_read,
            fs_cmds::fs_write,
            fs_cmds::fs_create,
            fs_cmds::fs_rename,
            fs_cmds::fs_trash,
            fs_cmds::fs_reveal,
            fs_cmds::quick_open_files,
            fs_cmds::search_files,
            fs_cmds::replace_all,
            fs_cmds::format_text,
            fs_cmds::head_text,
            fs_cmds::quick_edit,
            lsp::lsp_start,
            lsp::lsp_send,
            lsp::lsp_stop,
            settings::get_settings,
            settings::save_settings,
            settings::reveal_settings,
            worktree::worktree_finish,
            ssh::ssh_connect,
            ssh::ssh_disconnect,
            ssh::ssh_statuses,
            ssh::ssh_save,
            ssh::ssh_delete,
            ssh::ssh_master_pty,
            console_open,
            terminal_close,
            remote_terminal_open,
            sync::sync_configs,
            sync::sync_status,
            sync::sync_check,
            sync::sync_config_text,
            sync::sync_validate,
            sync::sync_save_config,
            sync::sync_set_paused,
            sync::sync_now,
            sync::sync_session_now,
            sync::gh_status,
            notify::notify_test,
            sync::sync_adopt_local,
            sync::load::sync_others,
            sync::load::sync_open_session,
            sync::load::sync_take_over,
            sync::load::sync_continue,
            sync::load::gh_repos,
            sync::load::sync_clone,
            sync::ship_cmds::ship_info,
            sync::ship_cmds::ship_run,
            sync::ship_cmds::ship_redeploy,
            sync::ship_cmds::ship_retry_deploy,
            sync::ship_cmds::ship_history,
            sync::ship_cmds::ship_undo,
            sync::fix::sync_fix_conflict,
            sync::repo::sync_create_repo,
            sync::ship_cmds::sync_workflows,
            sync::ship_cmds::sync_limit_workflows,
            set_window_appearance,
            sync::gh_terminal,
        ])
        .setup(move |app| {
            let (vib, dark, bg) = theme_window(&settings::theme());
            apply_window_appearance(app.handle(), vib, dark, bg);
            let handle = app.handle().clone();
            socket::serve(&socket_path, move |env| handle_hook(&handle, env))?;
            // Process definitions live in each project's .fleet/project.json.
            let detached = {
                let f = app.state::<Fleet>();
                let mut store = f.store.lock();
                for p in store.projects.iter_mut() {
                    if let Some(file) = processes::load_project_file(Path::new(&p.root_path)) {
                        p.processes = file.processes;
                    }
                }
                std::mem::take(&mut store.detached)
            };
            proc_runtime::adopt_detached(app.handle(), detached);
            // fleet.yml (synced projects take their processes from it) before anything starts.
            sync::refresh_configs(app.handle(), None);
            restore_sessions(app.handle());
            proc_runtime::autostart(app.handle(), None);
            proc_runtime::start_port_poller(app.handle().clone());
            ssh::start_poller(app.handle().clone());
            sync::start(app.handle().clone());
            // Auto-connect servers that ask for it (one master per server).
            let auto: Vec<String> = app.state::<Fleet>().store.lock().projects.iter().flat_map(|p| p.ssh_connections.iter().filter(|c| c.auto_connect).map(|c| c.id.clone())).collect();
            for id in auto {
                let _ = ssh::ssh_connect(app.handle().clone(), id, 120, 32);
            }
            {
                let f = app.state::<Fleet>();
                update_badge(app.handle(), &f.store.lock());
            }
            // Debounced persistence: at most every 2 s.
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(2));
                let f = handle.state::<Fleet>();
                if f.dirty.swap(false, Ordering::Relaxed) {
                    persist::save_store(&f.store.lock());
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let f = app.state::<Fleet>();
                if should_confirm_quit(&f) {
                    api.prevent_close();
                    let _ = app.emit("confirm-quit", proc_runtime::running_count(&f));
                }
            }
            if let WindowEvent::Focused(focused) = event {
                let app = window.app_handle();
                let f = app.state::<Fleet>();
                f.visible.lock().1 = *focused;
                if *focused {
                    sync::on_focus(app);
                    if let Some(id) = notify::take_recent(Duration::from_secs(30)) {
                        let visible = f.visible.lock().0.contains(&id);
                        if !visible {
                            let _ = app.emit("focus-session", id);
                        }
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building fleet");

    app.run(|app, event| match event {
        RunEvent::ExitRequested { api, .. } => {
            let f = app.state::<Fleet>();
            if should_confirm_quit(&f) {
                api.prevent_exit();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
                let _ = app.emit("confirm-quit", proc_runtime::running_count(&f));
            }
        }
        RunEvent::Exit => {
            // Sessions mid-prompt: wip commit + state + push before the PTYs go away.
            sync::wip_all(app, Duration::from_secs(8));
            sync::load::release_all(app, Duration::from_secs(3));
            let f = app.state::<Fleet>();
            persist::save_store(&f.store.lock());
            let leave = f.leave_processes_running.load(Ordering::SeqCst);
            f.ptys.kill_where(|id| !(leave && id.starts_with("proc:")));
            let _ = std::fs::remove_file(&f.socket_path);
            lsp::stop_all(&app.state::<lsp::LspState>());
        }
        _ => {}
    });
}

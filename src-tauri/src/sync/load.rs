//! Stage B: GitHub as the database. After every fetch, the sessions on fleet-state are
//! matched to this Mac's; the rest are listed "On other computers" and can be continued
//! here. Presence keeps one Mac per branch.
use super::*;
use crate::model::Session;
use presence::Holder;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherSession {
    pub project_id: String,
    pub branch: String,
    pub glyph: String,
    pub color: String,
    pub title: String,
    pub recap: Vec<String>,
    pub state: String,
    pub prompts: u32,
    pub last_machine: String,
    pub updated_at: String,
    pub commits: u32,
    /// Another Mac has it open right now (presence < 10 min old).
    pub active_on: Option<String>,
    pub active_ago: Option<String>,
    pub needs_rebase: bool,
}

pub fn install_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let p = persist::app_dir().join("install-id");
        if let Ok(s) = std::fs::read_to_string(&p) {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let _ = std::fs::write(&p, &id);
        id
    })
    .clone()
}

pub fn presence_ctx(app: &AppHandle, session_id: &str) -> autocommit::PresenceCtx {
    let st = app.state::<SyncState>();
    let expect = st.presence.lock().get(session_id).cloned();
    let take = st.take.lock().remove(session_id);
    let rewritten_from = st.rewrite.lock().get(session_id).cloned();
    autocommit::PresenceCtx { machine: machine_name(), install: install_id(), expect, take, rewritten_from }
}

pub fn remember_presence(app: &AppHandle, session_id: &str, tag: Option<String>) {
    if let Some(t) = tag {
        let st = app.state::<SyncState>();
        st.presence.lock().insert(session_id.to_string(), t);
        st.presence_at.lock().insert(session_id.to_string(), now_ms());
    }
}

fn stamp(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).map(|d| d.timestamp_millis()).unwrap_or(0)
}

/// Push fleet-state alone (after archiving files etc.), replaying on rejection.
pub fn push_state(git: &Git) {
    for _ in 0..3 {
        match pusher::push(git, &[pusher::Refspec::branch(state_branch::BRANCH)], &[], None) {
            pusher::PushOutcome::Rejected(..) => {
                let _ = git.run(&["fetch", "origin", "+refs/heads/fleet-state:refs/remotes/origin/fleet-state"]);
                if state_branch::replay(git).is_err() {
                    return;
                }
            }
            _ => return,
        }
    }
}

/// The load sequence (after each fetch): update local sessions from their files, list the
/// others, archive files whose branch is gone, fast-forward the main folder.
pub fn load_sequence(app: &AppHandle, p: &Project) {
    let Some(c) = cfg(app, &p.id).filter(|c| c.enabled) else { return };
    let prefix = c.config.as_ref().map(|x| x.sync.branch_prefix.clone()).unwrap_or_else(|| "fleet/".into());
    let git = git_for(app, &p.root_path);
    let files = state_branch::read_sessions(&git, state_branch::REMOTE);
    let remote_branches: HashSet<String> = git.read(&["for-each-ref", "--format=%(refname:strip=3)", "refs/remotes/origin/"]).unwrap_or_default().lines().map(String::from).collect();
    let pres: HashMap<String, presence::Presence> = presence::all(&git).into_iter().collect();
    let me = install_id();
    let now = now_ms();
    let main_ref = format!("refs/remotes/origin/{}", c.default_branch);
    let local: Vec<Session> = app.state::<Fleet>().store.lock().sessions.iter().filter(|s| s.project_id == p.id && !s.archived && !s.branch.is_empty()).cloned().collect();
    let mut others = vec![];
    let mut archive = vec![];
    for (path, f) in files {
        if f.archived {
            continue;
        }
        let mine = local.iter().find(|s| s.branch == f.branch);
        if !remote_branches.contains(&f.branch) {
            // Merged (shipped) or deleted on GitHub.
            let mut g = f.clone();
            g.archived = true;
            g.updated_at = state_branch::now_stamp();
            archive.push((path.clone(), Some(state_branch::to_yaml(&g))));
            if let Some(s) = mine {
                detach_shipped(app, s, &c.default_branch);
            }
            continue;
        }
        match mine {
            Some(s) => {
                let seen = s.sync.as_ref().and_then(|i| i.remote_updated_at.clone()).unwrap_or_default();
                if stamp(&f.updated_at) > stamp(&seen) {
                    let f2 = f.clone();
                    crate::update_session(app, &s.id, |x| {
                        let info = x.sync.get_or_insert_with(Default::default);
                        info.remote_updated_at = Some(f2.updated_at.clone());
                        if x.state == SessionState::Working || f2.last_machine == machine_name() {
                            return true;
                        }
                        if !x.title_locked && !f2.title.is_empty() && x.title != f2.title {
                            x.title = f2.title.clone();
                            x.title_history = f2.title_history.clone();
                        }
                        if !f2.recap.is_empty() {
                            x.recap = f2.recap.clone();
                        }
                        if !x.pinned_glyph {
                            x.glyph = f2.glyph.clone();
                        }
                        if !x.pinned_color {
                            x.color = f2.color.clone();
                        }
                        x.prompt_count = x.prompt_count.max(f2.prompts);
                        true
                    });
                }
                if f.needs_rebase {
                    let holder = presence::holder(pres.get(&f.branch), &me, now);
                    if matches!(holder, Holder::Mine | Holder::Nobody | Holder::Stale { .. }) {
                        let (app, id) = (app.clone(), s.id.clone());
                        std::thread::spawn(move || rebase_after_ship(&app, &id));
                    }
                }
            }
            None => {
                let (active_on, active_ago) = match presence::holder(pres.get(&f.branch), &me, now) {
                    Holder::Other { machine, age_ms } => (Some(machine), Some(presence::ago(age_ms))),
                    _ => (None, None),
                };
                let commits = git.read(&["rev-list", "--count", &format!("{main_ref}..refs/remotes/origin/{}", f.branch)]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                let _ = &prefix;
                others.push(OtherSession {
                    project_id: p.id.clone(),
                    branch: f.branch.clone(),
                    glyph: f.glyph.clone(),
                    color: f.color.clone(),
                    title: f.title.clone(),
                    recap: f.recap.clone(),
                    state: f.state.clone(),
                    prompts: f.prompts,
                    last_machine: f.last_machine.clone(),
                    updated_at: f.updated_at.clone(),
                    commits,
                    active_on,
                    active_ago,
                    needs_rebase: f.needs_rebase,
                });
            }
        }
    }
    // Presence left behind for branches that are gone (shipped/deleted): any Mac removes it.
    if !p.sync_paused && !paused_all() {
        let stale: Vec<pusher::Refspec> = pres.keys().filter(|b| !remote_branches.contains(*b)).map(|b| pusher::Refspec::delete(&presence::remote_ref(b))).collect();
        let leases: Vec<pusher::Lease> = pres.iter().filter(|(b, _)| !remote_branches.contains(*b)).map(|(b, pr)| pusher::Lease { dst: presence::remote_ref(b), expect: pr.sha.clone() }).collect();
        if !stale.is_empty() {
            let _ = pusher::push(&git, &stale, &leases, None);
        }
    }
    if !archive.is_empty() && !p.sync_paused && !paused_all() {
        if state_branch::write(&git, &archive, &format!("state: archive {} merged/deleted branch(es)", archive.len())).is_ok() {
            push_state(&git);
        }
    }
    others.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    app.state::<SyncState>().others.lock().insert(p.id.clone(), others);
    emit_others(app);
    fast_forward_main(app, p, &git, &c.default_branch);
}

pub fn emit_others(app: &AppHandle) {
    let all: Vec<OtherSession> = app.state::<SyncState>().others.lock().values().flatten().cloned().collect();
    let _ = app.emit("sync-others", &all);
}

/// The main folder follows GitHub's main when it has no local work. Dev processes running
/// from it restart if dependencies changed.
fn fast_forward_main(app: &AppHandle, p: &Project, git: &Git, default_branch: &str) {
    let work = adopt::scan(git, default_branch);
    if !work.files.is_empty() || work.branch.as_deref() != Some(default_branch) {
        return;
    }
    let origin = format!("refs/remotes/origin/{default_branch}");
    let (Some(head), Some(o)) = (work.head.clone(), git.rev(&origin)) else { return };
    if head == o || !git.is_ancestor(&head, &o) {
        return;
    }
    let changed = git.read(&["diff", "--name-only", &head, &o]).unwrap_or_default();
    if git.run(&["merge", "-q", "--ff-only", &origin]).is_err() {
        return;
    }
    let deps = ["package.json", "package-lock.json", "pnpm-lock.yaml", "yarn.lock", "bun.lockb", "bun.lock", "Cargo.lock", "go.sum", "Gemfile.lock", "poetry.lock", "uv.lock", "requirements.txt", "pyproject.toml", "composer.lock"];
    let deps_changed = changed.lines().any(|l| deps.iter().any(|d| l == *d || l.ends_with(&format!("/{d}"))));
    if deps_changed {
        let runs: Vec<(String, String)> = app.state::<Fleet>().runs.lock().values().filter(|r| r.project_id == p.id && r.status == crate::processes::RunStatus::Running && r.session_id.is_none()).map(|r| (r.project_id.clone(), r.def_id.clone())).collect();
        for (pid, did) in runs {
            let _ = crate::proc_runtime::restart(app, &pid, &did, 120, 32, None);
        }
        if !app.state::<Fleet>().runs.lock().is_empty() {
            let _ = app.emit("toast", format!("{} updated from GitHub; dependencies changed, dev processes restarted.", p.name));
        }
    }
}

/// The session's branch was shipped (merged + deleted): its worktree goes back to a clean
/// detached main, and its next prompt starts a new branch.
pub fn detach_shipped(app: &AppHandle, s: &Session, default_branch: &str) {
    let Some(wt) = s.worktree_path.clone().filter(|w| Path::new(w).is_dir()) else { return };
    if s.state == SessionState::Working {
        return;
    }
    let git = git_for(app, &wt);
    let dirty = git.read(&["status", "--porcelain", "--untracked-files=no"]).map(|o| !o.trim().is_empty()).unwrap_or(true);
    let branch = s.branch.clone();
    if dirty || git.run(&["switch", "-q", "--detach", &format!("origin/{default_branch}")]).is_err() {
        set_sync(app, &s.id, |i| {
            i.badge = "paused".into();
            i.detail = Some(format!("{branch} was shipped, but this worktree has uncommitted changes. Finish the prompt; fleet moves it to a new branch after."));
        });
        return;
    }
    let _ = git.run(&["branch", "-q", "-D", &branch]);
    if let Some(mine) = app.state::<SyncState>().presence.lock().remove(&s.id) {
        autocommit::release_presence(&git, &branch, &mine);
    }
    crate::update_session(app, &s.id, |x| {
        x.branch = String::new();
        x.files_touched.clear();
        x.line_marks.clear();
        x.sync = Some(SyncInfo { badge: "new".into(), detail: Some("Shipped ✓ — the next prompt starts a new branch".into()), ..Default::default() });
        true
    });
}

/// Main moved under this branch (Ship): rebase onto it and push with a lease. Only the Mac
/// holding the branch does this; a conflict is left for the session to resolve.
pub fn rebase_after_ship(app: &AppHandle, id: &str) {
    let Some((s, p)) = find(app, id) else { return };
    let Some(c) = cfg(app, &p.id).filter(|c| c.enabled) else { return };
    let Some(wt) = s.worktree_path.clone() else { return };
    if s.state == SessionState::Working || s.branch.is_empty() {
        return;
    }
    let lock = session_lock(app, id);
    let _g = lock.lock();
    let git = git_for(app, &wt);
    if git.operation_in_progress().is_some() {
        return;
    }
    let main = format!("origin/{}", c.default_branch);
    let old_remote = git.rev(&format!("refs/remotes/origin/{}", s.branch)).unwrap_or_default();
    if git.run(&["rebase", "--autostash", &main]).is_err() {
        let files = autocommit::conflicted(&git);
        let q = format!("main moved (a Ship). Rebasing {} onto it conflicts in {} — resolve the conflicts and run `git rebase --continue`.", s.branch, files.join(", "));
        crate::update_session(app, id, |x| {
            x.state = SessionState::NeedsReply;
            x.state_since = now_ms();
            x.pending_question = Some(q.clone());
            true
        });
        set_sync(app, id, |i| {
            i.badge = "conflict".into();
            i.detail = Some(format!("Conflict with main in {}", files.join(", ")));
        });
        return;
    }
    let ctx = presence_ctx(app, id);
    let tag = git.rev(&format!("refs/heads/{}", s.branch)).and_then(|h| presence::make_tag(&git, &h, &ctx.machine, &ctx.install, chrono::Utc::now().timestamp()).ok());
    let mut specs = vec![pusher::Refspec::branch(&s.branch)];
    let mut leases = vec![pusher::Lease { dst: format!("refs/heads/{}", s.branch), expect: old_remote }];
    if let Some(t) = &tag {
        specs.push(pusher::Refspec::new(t, &presence::remote_ref(&s.branch)));
        leases.push(pusher::Lease { dst: presence::remote_ref(&s.branch), expect: ctx.expect.clone().or_else(|| presence::fetched(&git, &s.branch).map(|p| p.sha)).unwrap_or_default() });
    }
    match pusher::push(&git, &specs, &leases, None) {
        pusher::PushOutcome::Pushed => {
            remember_presence(app, id, tag);
            let path = state_branch::file_for(&s.branch, &c.config.as_ref().map(|x| x.sync.branch_prefix.clone()).unwrap_or_else(|| "fleet/".into()));
            if let Some(mut f) = state_branch::read_session(&git, state_branch::REMOTE, &path) {
                f.needs_rebase = false;
                f.updated_at = state_branch::now_stamp();
                if state_branch::write(&git, &[(path, Some(state_branch::to_yaml(&f)))], &format!("state: {} rebased onto main", s.glyph)).is_ok() {
                    push_state(&git);
                }
            }
            set_sync(app, id, |i| {
                i.badge = "pushed".into();
                i.detail = Some("Rebased onto the new main".into());
                i.last_push_at = Some(now_ms());
            });
        }
        o => set_sync(app, id, |i| {
            i.badge = "error".into();
            i.detail = Some(format!("Couldn't push the rebased branch: {o:?}"));
        }),
    }
}

/// Every 5 minutes: refresh presence for this Mac's open synced sessions.
pub fn refresh_presence(app: &AppHandle) {
    let now = now_ms();
    let open: Vec<(String, String, String, String)> = {
        let f = app.state::<Fleet>();
        let store = f.store.lock();
        store
            .sessions
            .iter()
            .filter(|s| s.alive && !s.archived && !s.branch.is_empty() && s.remote.is_none() && s.worktree_path.is_some())
            .filter(|s| !matches!(s.sync.as_ref().map(|i| i.badge.as_str()), Some("taken_over") | Some("conflict") | Some("new") | None))
            .filter_map(|s| store.projects.iter().find(|p| p.id == s.project_id && !p.sync_paused).map(|_| (s.id.clone(), s.project_id.clone(), s.branch.clone(), s.worktree_path.clone().unwrap())))
            .collect()
    };
    if paused_all() {
        return;
    }
    for (id, pid, branch, wt) in open {
        if enabled_cfg(app, &pid).is_none() {
            continue;
        }
        let last = app.state::<SyncState>().presence_at.lock().get(&id).copied().unwrap_or(0);
        if now - last < presence::REFRESH_MS {
            continue;
        }
        app.state::<SyncState>().presence_at.lock().insert(id.clone(), now);
        let git = git_for(app, &wt);
        let ctx = presence_ctx(app, &id);
        let (st, tag) = autocommit::push_presence(&git, &branch, &ctx);
        match st {
            autocommit::Status::Pushed => remember_presence(app, &id, tag),
            autocommit::Status::TakenOver(m) => set_sync(app, &id, |i| {
                i.badge = "taken_over".into();
                i.detail = Some(m.clone());
            }),
            _ => {}
        }
    }
}

/// Session closed (or fleet quitting): drop our presence so another Mac continues freely.
pub fn release(app: &AppHandle, id: &str) {
    let Some(mine) = app.state::<SyncState>().presence.lock().remove(id) else { return };
    let Some((s, _)) = find(app, id) else { return };
    let (Some(wt), false) = (s.worktree_path.clone(), s.branch.is_empty()) else { return };
    let git = git_for(app, &wt);
    autocommit::release_presence(&git, &s.branch, &mine);
}

pub fn release_all(app: &AppHandle, limit: Duration) {
    let ids: Vec<String> = app.state::<SyncState>().presence.lock().keys().cloned().collect();
    if ids.is_empty() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    for id in ids.clone() {
        let (app, tx) = (app.clone(), tx.clone());
        std::thread::spawn(move || {
            release(&app, &id);
            let _ = tx.send(());
        });
    }
    let deadline = std::time::Instant::now() + limit;
    for _ in 0..ids.len() {
        if rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())).is_err() {
            break;
        }
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn sync_others(app: AppHandle) -> Vec<OtherSession> {
    app.state::<SyncState>().others.lock().values().flatten().cloned().collect()
}

/// Opening a session: fast-forward its branch if GitHub has newer commits (≤ every 30 s),
/// and see whether another Mac holds it.
#[tauri::command]
pub fn sync_open_session(app: AppHandle, session_id: String) {
    let now = now_ms();
    {
        let st = app.state::<SyncState>();
        let mut opened = st.opened.lock();
        if now - opened.get(&session_id).copied().unwrap_or(0) < 30_000 {
            return;
        }
        opened.insert(session_id.clone(), now);
    }
    std::thread::spawn(move || {
        let Some((s, p)) = find(&app, &session_id) else { return };
        if s.branch.is_empty() || s.state == SessionState::Working || enabled_cfg(&app, &p.id).is_none() {
            return;
        }
        let Some(wt) = s.worktree_path.clone() else { return };
        let git = git_for(&app, &wt);
        if git.run(&["fetch", "-q", "origin", &format!("+refs/heads/{0}:refs/remotes/origin/{0}", s.branch)]).is_err() {
            return;
        }
        let lock = session_lock(&app, &session_id);
        let _g = lock.lock();
        let (local, remote) = (git.rev("HEAD"), git.rev(&format!("refs/remotes/origin/{}", s.branch)));
        if let (Some(l), Some(r)) = (local, remote) {
            if l != r && git.is_ancestor(&l, &r) && git.operation_in_progress().is_none() {
                let _ = git.run(&["merge", "-q", "--ff-only", &format!("origin/{}", s.branch)]);
            }
        }
        let cur = presence::fetch_one(&git, &s.branch);
        if let Holder::Other { machine, age_ms } = presence::holder(cur.as_ref(), &install_id(), now_ms()) {
            set_sync(&app, &session_id, |i| {
                i.badge = "taken_over".into();
                i.detail = Some(format!("Active on {machine} · {}", presence::ago(age_ms)));
            });
        }
    });
}

/// "Take over" a session another Mac holds: the next push replaces its presence.
#[tauri::command]
pub fn sync_take_over(app: AppHandle, session_id: String) {
    app.state::<SyncState>().take.lock().insert(session_id.clone());
    std::thread::spawn(move || {
        let Some((s, _)) = find(&app, &session_id) else { return };
        let Some(wt) = s.worktree_path.clone() else { return };
        if s.branch.is_empty() {
            return;
        }
        let git = git_for(&app, &wt);
        let _ = git.run(&["fetch", "-q", "origin", &format!("+refs/heads/{0}:refs/remotes/origin/{0}", s.branch)]);
        // Catch up with what the other Mac pushed, then claim the branch.
        let (l, r) = (git.rev("HEAD"), git.rev(&format!("refs/remotes/origin/{}", s.branch)));
        if let (Some(l), Some(r)) = (l, r) {
            if l != r && git.is_ancestor(&l, &r) {
                let _ = git.run(&["merge", "-q", "--ff-only", &format!("origin/{}", s.branch)]);
            }
        }
        let ctx = presence_ctx(&app, &session_id);
        let ctx = autocommit::PresenceCtx { take: true, ..ctx };
        let (st, tag) = autocommit::push_session(&git, &s.branch, Some(&ctx));
        remember_presence(&app, &session_id, tag);
        set_sync(&app, &session_id, |i| match &st {
            autocommit::Status::Pushed => {
                i.badge = "pushed".into();
                i.detail = Some("Taken over — this Mac holds it now".into());
            }
            other => {
                i.badge = "error".into();
                i.detail = Some(format!("{other:?}"));
            }
        });
    });
}

/// "Continue here": a session from another Mac gets a worktree on its branch and a new
/// claude session here with the same identity, primed with its recap (or its synced chat).
#[tauri::command]
pub async fn sync_continue(app: AppHandle, project_id: String, branch: String, take: bool, cols: u16, rows: u16) -> Result<Session, String> {
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let c = cfg(&app, &p.id).filter(|c| c.enabled).ok_or("GitHub sync isn't on for this project.")?;
        let conf = c.config.clone().unwrap_or_else(|| fleetyml::parse("project: x\n").unwrap());
        let prefix = conf.sync.branch_prefix.clone();
        let root = PathBuf::from(&p.root_path);
        let git = git_for(&app, &root);
        git.run(&["fetch", "-q", "origin", &format!("+refs/heads/{0}:refs/remotes/origin/{0}", branch), "+refs/heads/fleet-state:refs/remotes/origin/fleet-state"]).map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
        let path = state_branch::file_for(&branch, &prefix);
        let f = state_branch::read_session(&git, state_branch::REMOTE, &path).ok_or("That session's file isn't on GitHub anymore.")?;
        if !take {
            if let Holder::Other { machine, age_ms } = presence::holder(presence::fetch_one(&git, &branch).as_ref(), &install_id(), now_ms()) {
                return Err(format!("ACTIVE:Active on {machine} · {}", presence::ago(age_ms)));
            }
        }
        let stem = path.trim_start_matches("sessions/").trim_end_matches(".yml").to_string();
        let wt_s = worktree_on_branch(&git, &root, &branch, &stem)?;
        let wt = PathBuf::from(&wt_s);
        let wgit = git.at(&wt);
        if let Some(setup) = conf.sync.setup.clone().filter(|s| !s.trim().is_empty()) {
            run_setup(&app, &wt_s, &setup);
        }
        // Synced chat → resume the same conversation; otherwise prime with the recap.
        let mut resume_id = None;
        if let (Some(cid), Some(t)) = (f.claude_session_id.clone(), git.show_file(state_branch::REMOTE, &state_branch::transcript_for(&path))) {
            let dir = adopt::claude_project_dir(&wt_s);
            if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(dir.join(format!("{cid}.jsonl")), &t).is_ok() {
                let file = dir.join(format!("{cid}.jsonl"));
                resume_id = Some((cid, file));
            }
        }
        let prompt = if resume_id.is_some() {
            None
        } else {
            let log = wgit.read(&["log", "--format=- %s", &format!("origin/{}..HEAD", c.default_branch)]).unwrap_or_default();
            let stat = wgit.read(&["diff", "--stat", &format!("origin/{}...HEAD", c.default_branch)]).unwrap_or_default();
            let recap = f.recap.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join("\n");
            Some(format!(
                "I'm continuing this session on another computer (it was last on {}). Here's where it stood.\n\nTitle: {}\n\nRecap:\n{}\n\nCommits on this branch:\n{}\nChanges vs main:\n{}\nRead what changed so you're up to date, then tell me in a few lines where we are. Don't change anything until I ask.",
                f.last_machine,
                f.title,
                if recap.is_empty() { "(none)".into() } else { recap },
                if log.trim().is_empty() { "(none yet)\n".to_string() } else { log },
                if stat.trim().is_empty() { "(none)\n".to_string() } else { stat },
            ))
        };
        let s = crate::create_session(
            &app,
            &p,
            crate::SessionSpec {
                title: f.title.clone(),
                prompt,
                worktree_path: Some(wt_s.clone()),
                branch: branch.clone(),
                glyph: Some(f.glyph.clone()),
                color: Some(f.color.clone()),
                title_history: f.title_history.clone(),
                recap: f.recap.clone(),
                prompt_count: f.prompts,
                claude_session_id: resume_id.as_ref().map(|r| r.0.clone()),
                transcript_path: resume_id.as_ref().map(|r| r.1.to_string_lossy().to_string()),
                sync: Some(SyncInfo { badge: "syncing".into(), remote_updated_at: Some(f.updated_at.clone()), ..Default::default() }),
                ..Default::default()
            },
            cols,
            rows,
        )?;
        // Claim the branch on GitHub.
        let ctx = autocommit::PresenceCtx { machine: machine_name(), install: install_id(), expect: None, take: true, rewritten_from: None };
        let (st, tag) = autocommit::push_presence(&wgit, &branch, &ctx);
        remember_presence(&app, &s.id, tag);
        set_sync(&app, &s.id, |i| {
            i.badge = if matches!(st, autocommit::Status::Pushed) { "pushed".into() } else { "waiting".into() };
            i.detail = Some(format!("Continued from {}", f.last_machine));
        });
        if f.needs_rebase {
            let (app, id) = (app.clone(), s.id.clone());
            std::thread::spawn(move || rebase_after_ship(&app, &id));
        }
        app.state::<SyncState>().others.lock().entry(p.id.clone()).or_default().retain(|o| o.branch != branch);
        emit_others(&app);
        Ok(find(&app, &s.id).map(|x| x.0).unwrap_or(s))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A worktree under .fleet/worktrees on `branch`, tracking GitHub's copy (fast-forwarded to it
/// when the local branch is behind). Reuses the worktree that already has it checked out.
pub fn worktree_on_branch(git: &Git, root: &Path, branch: &str, stem: &str) -> Result<String, String> {
    let origin_b = format!("origin/{branch}");
    let existing = git.read(&["worktree", "list", "--porcelain"]).unwrap_or_default();
    let mut path = None;
    for line in existing.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
        } else if line == format!("branch refs/heads/{branch}") {
            if let Some(p) = path.filter(|p| Path::new(p).is_dir() && Path::new(p) != root) {
                return Ok(p);
            }
            return Err(format!("{branch} is checked out in the project's main folder. Switch that folder back to main first."));
        }
    }
    let base = root.join(".fleet").join("worktrees");
    let slug = crate::worktree::slugify(stem, &|c| base.join(c).exists());
    let wt = base.join(&slug);
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    crate::processes::exclude_locally(root, ".fleet/").map_err(|e| e.to_string())?;
    let wt_s = wt.to_string_lossy().to_string();
    match git.rev(&format!("refs/heads/{branch}")) {
        Some(l) => {
            let r = git.rev(&format!("refs/remotes/{origin_b}")).unwrap_or_default();
            if l != r && git.is_ancestor(&l, &r) {
                let _ = git.run(&["branch", "-f", branch, &origin_b]);
            }
            git.run(&["worktree", "add", "-q", &wt_s, branch]).map_err(|e| e.to_string())?;
        }
        None => {
            git.run(&["worktree", "add", "-q", "--track", "-b", branch, &wt_s, &origin_b]).map_err(|e| e.to_string())?;
        }
    }
    let _ = git.at(&wt).read(&["branch", "--set-upstream-to", &origin_b, branch]);
    Ok(wt_s)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhRepo {
    pub name_with_owner: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub is_private: bool,
    #[serde(default)]
    pub updated_at: String,
}

/// The user's GitHub repos (gh repo list).
#[tauri::command]
pub async fn gh_repos() -> Result<Vec<GhRepo>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let gh = gh::bin().ok_or("Install the GitHub CLI first (GitHub sync… in a project's menu).")?;
        let out = std::process::Command::new(gh)
            .args(["repo", "list", "--limit", "200", "--json", "nameWithOwner,description,isPrivate,updatedAt"])
            .env_clear()
            .envs(crate::shell_env::login_env())
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        let mut v: Vec<GhRepo> = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(v)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Clone a GitHub repo into `parent` (or reuse an existing clone there) and add it.
#[tauri::command]
pub async fn sync_clone(app: AppHandle, repo: String, parent: String) -> Result<Project, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = repo.rsplit('/').next().unwrap_or(&repo).to_string();
        if name.is_empty() || name.contains("..") {
            return Err("bad repo name".into());
        }
        let dest = PathBuf::from(&parent).join(&name);
        if !dest.join(".git").exists() {
            if dest.exists() && std::fs::read_dir(&dest).map(|mut d| d.next().is_some()).unwrap_or(false) {
                return Err(format!("{} already exists and isn't a git checkout.", dest.display()));
            }
            let gh = gh::bin().ok_or("Install the GitHub CLI first.")?;
            let t0 = std::time::Instant::now();
            let out = std::process::Command::new(gh).args(["repo", "clone", &repo]).arg(&dest).env_clear().envs(crate::shell_env::login_env()).output().map_err(|e| e.to_string())?;
            logger(&app)(LogLine { root: dest.to_string_lossy().into(), cmd: format!("gh repo clone {repo}"), output: String::from_utf8_lossy(&out.stderr).trim().to_string(), code: out.status.code().unwrap_or(-1), ms: t0.elapsed().as_millis() as u64 });
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
        }
        let p = crate::add_project(dest.to_string_lossy().to_string(), app.state::<Fleet>(), app.clone())?;
        refresh_configs(&app, Some(&p.id));
        if let Some(pp) = app.state::<Fleet>().store.lock().projects.iter().find(|x| x.id == p.id).cloned() {
            fetch_project(&app, &pp);
        }
        Ok(p)
    })
    .await
    .map_err(|e| e.to_string())?
}

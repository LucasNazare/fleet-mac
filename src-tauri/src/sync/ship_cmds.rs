//! Tauri side of Ship: ready branches, the run (with live log), redeploy, undo, the GitHub
//! Actions cost guard.
use super::ship::{self, BranchResult, ShipBranch};
use super::*;
use std::io::Read;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub branch: String,
    pub title: String,
    pub glyph: String,
    pub color: String,
    pub commits: u32,
    pub stat: String,
    pub machine: String,
    pub state: String,
    pub ready: bool,
    pub reason: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShipInfo {
    pub candidates: Vec<Candidate>,
    pub checks: Vec<String>,
    pub deploy_command: Option<String>,
    pub merge_style: String,
    pub protected: bool,
    pub default_branch: String,
    pub deploys: Vec<state_branch::DeployEntry>,
    pub shipping: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShippedCommit {
    pub commit: String,
    pub subject: String,
    pub at: i64,
    pub branch: String,
}

fn shipping() -> &'static Mutex<HashSet<String>> {
    static S: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Branches that can ship: on GitHub, ahead of main, done, everything pushed.
#[tauri::command]
pub async fn ship_info(app: AppHandle, project_id: String) -> Result<ShipInfo, String> {
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let c = cfg(&app, &p.id).filter(|c| c.enabled).ok_or("GitHub sync isn't on for this project.")?;
        let conf = c.config.clone().ok_or("fleet.yml is missing.")?;
        let git = git_for(&app, &p.root_path);
        let _ = git.run(&["fetch", "--prune", "origin", "+refs/heads/*:refs/remotes/origin/*", presence::FETCH_SPEC]);
        let main_ref = format!("refs/remotes/origin/{}", c.default_branch);
        let me = load::install_id();
        let pres: HashMap<String, presence::Presence> = presence::all(&git).into_iter().collect();
        let local: Vec<Session> = app.state::<Fleet>().store.lock().sessions.iter().filter(|s| s.project_id == p.id && !s.archived).cloned().collect();
        let queue: HashSet<String> = app.state::<SyncState>().queue.lock().iter().map(|q| q.branch.clone()).collect();
        let mut out = vec![];
        let mut seen = HashSet::new();
        let ahead = |b: &str| -> u32 { git.read(&["rev-list", "--count", &format!("{main_ref}..refs/remotes/origin/{b}")]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0) };
        let stat = |b: &str| git.read(&["diff", "--shortstat", &format!("{main_ref}...refs/remotes/origin/{b}")]).unwrap_or_default().trim().to_string();
        for (_, f) in state_branch::read_sessions(&git, state_branch::REMOTE) {
            if f.archived || git.rev(&format!("refs/remotes/origin/{}", f.branch)).is_none() {
                continue;
            }
            seen.insert(f.branch.clone());
            let commits = ahead(&f.branch);
            let s = local.iter().find(|s| s.branch == f.branch);
            let mut reason = None;
            if commits == 0 {
                reason = Some("Nothing new vs main".to_string());
            } else if let Some(s) = s {
                let local_head = git.rev(&format!("refs/heads/{}", f.branch));
                let remote_head = git.rev(&format!("refs/remotes/origin/{}", f.branch));
                if s.alive && s.state == SessionState::Working {
                    reason = Some("Working".into());
                } else if queue.contains(&f.branch) || local_head != remote_head {
                    reason = Some("Not pushed yet".into());
                } else if matches!(s.sync.as_ref().map(|i| i.badge.as_str()), Some("conflict")) {
                    reason = Some("Has a conflict".into());
                }
            } else if f.state == "working" {
                if let presence::Holder::Other { machine, .. } = presence::holder(pres.get(&f.branch), &me, now_ms()) {
                    reason = Some(format!("Working on {machine}"));
                }
            }
            out.push(Candidate {
                branch: f.branch.clone(),
                title: f.title.clone(),
                glyph: f.glyph.clone(),
                color: f.color.clone(),
                commits,
                stat: stat(&f.branch),
                machine: f.last_machine.clone(),
                state: s.map(|s| serde_json::to_value(s.state).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()).unwrap_or(f.state.clone()),
                ready: reason.is_none(),
                reason,
                session_id: s.map(|s| s.id.clone()),
            });
        }
        // Settings changes (fleet.yml, workflow limits) and undo branches ship too.
        let prefix = conf.sync.branch_prefix.clone();
        for b in git.read(&["for-each-ref", "--format=%(refname:strip=3)", "refs/remotes/origin/"]).unwrap_or_default().lines() {
            let is_cfg = b.starts_with(&format!("{prefix}config-"));
            if !(is_cfg || b.starts_with(&format!("{prefix}revert-"))) || seen.contains(b) {
                continue;
            }
            let commits = ahead(b);
            if commits == 0 {
                continue;
            }
            let subject = git.read(&["log", "-1", "--format=%s", &format!("refs/remotes/origin/{b}")]).unwrap_or_default().trim().to_string();
            out.push(Candidate {
                branch: b.to_string(),
                title: if is_cfg { "Settings (fleet.yml)".into() } else { subject },
                glyph: if is_cfg { "⚙".into() } else { "↶".into() },
                color: "#8B949E".into(),
                commits,
                stat: stat(b),
                machine: String::new(),
                state: "done".into(),
                ready: true,
                reason: None,
                session_id: None,
            });
        }
        out.sort_by_key(|c| !c.ready);
        Ok(ShipInfo {
            candidates: out,
            checks: conf.sync.checks.clone(),
            deploy_command: conf.sync.deploy_command.clone().filter(|d| !d.trim().is_empty()),
            merge_style: conf.sync.merge_style.clone(),
            protected: conf.sync.main_is_protected,
            default_branch: c.default_branch.clone(),
            deploys: state_branch::read_deploys(&git, state_branch::REMOTE),
            shipping: shipping().lock().contains(&p.id),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Shell command with the login env; output streamed as `ship-log` events.
/// Longest a check, setup or deploy command may run before Ship stops it.
const SHIP_CMD_TIMEOUT: Duration = Duration::from_secs(45 * 60);

fn streaming_runner(app: &AppHandle, project_id: &str) -> impl Fn(&Path, &str) -> (i32, String) {
    let (app, pid) = (app.clone(), project_id.to_string());
    move |dir: &Path, cmd: &str| {
        let emit = |t: &str| {
            let _ = app.emit("ship-log", serde_json::json!({ "projectId": pid, "text": t }));
        };
        emit(&format!("\n$ {cmd}\n"));
        let shell = crate::shell_env::login_env().get("SHELL").cloned().unwrap_or_else(|| "/bin/zsh".into());
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new(shell)
            .arg("-c")
            .arg(format!("exec 2>&1; {cmd}"))
            .current_dir(dir)
            .env_clear()
            .envs(crate::shell_env::login_env())
            .env("CI", "1")
            .env("FLEET_SHIP", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            // Its own process group: a timeout stops everything it started.
            .process_group(0)
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                emit(&format!("{e}\n"));
                return (-1, e.to_string());
            }
        };
        // Output is read on its own thread: something the command left running in the
        // background (a daemon, `nohup … &`) can hold the pipe open after the command ends,
        // and that mustn't keep Ship waiting forever.
        let mut stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 || tx.send(String::from_utf8_lossy(&buf[..n]).to_string()).is_err() {
                    break;
                }
            }
        });
        let started = std::time::Instant::now();
        let mut all = String::new();
        let mut exited: Option<(i32, std::time::Instant)> = None;
        let mut timed_out = false;
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(t) => {
                    emit(&t);
                    all.push_str(&t);
                    if all.len() > 200_000 {
                        let mut cut = 100_000;
                        while !all.is_char_boundary(cut) {
                            cut += 1;
                        }
                        all.drain(..cut);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            if exited.is_none() {
                if let Ok(Some(st)) = child.try_wait() {
                    exited = Some((st.code().unwrap_or(-1), std::time::Instant::now()));
                }
            }
            match exited {
                // Exited; give its last output 3 s, then stop reading.
                Some((_, at)) if at.elapsed() > Duration::from_secs(3) => break,
                None if started.elapsed() > SHIP_CMD_TIMEOUT => {
                    timed_out = true;
                    unsafe {
                        libc::killpg(child.id() as i32, libc::SIGTERM);
                    }
                    std::thread::sleep(Duration::from_secs(5));
                    unsafe {
                        libc::killpg(child.id() as i32, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    break;
                }
                _ => {}
            }
        }
        let code = match exited {
            Some((c, _)) => c,
            None if timed_out => {
                emit(&format!("\n[stopped after {} min]\n", SHIP_CMD_TIMEOUT.as_secs() / 60));
                -2
            }
            None => child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1),
        };
        emit(&format!("[exit {code}]\n"));
        (code, all)
    }
}

fn pr_merge(app: &AppHandle, project_id: &str) -> impl Fn(&Git, &str, &str) -> Result<(), String> {
    let (app, pid) = (app.clone(), project_id.to_string());
    move |g: &Git, head: &str, titles: &str| {
        let gh = gh::bin().ok_or("mainIsProtected needs the GitHub CLI (gh).")?;
        let ghc = |args: &[&str]| {
            let o = std::process::Command::new(&gh).args(args).current_dir(&g.root).env_clear().envs(crate::shell_env::login_env()).output().map_err(|e| e.to_string())?;
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            let _ = app.emit("ship-log", serde_json::json!({ "projectId": pid, "text": format!("$ gh {}\n{text}", args.join(" ")) }));
            if o.status.success() { Ok(text) } else { Err(text) }
        };
        let base = g.default_branch();
        ghc(&["pr", "create", "--base", &base, "--head", head, "--title", &format!("Ship: {titles}"), "--body", "Shipped with fleet. One commit per feature; checks ran locally."])?;
        ghc(&["pr", "merge", head, "--auto", "--rebase", "--delete-branch"])?;
        for _ in 0..180 {
            let state = ghc(&["pr", "view", head, "--json", "state", "-q", ".state"]).unwrap_or_default();
            match state.trim() {
                "MERGED" => return Ok(()),
                "CLOSED" => return Err("The pull request was closed without merging.".into()),
                _ => std::thread::sleep(Duration::from_secs(10)),
            }
        }
        // Not merged in time: disarm auto-merge so it can't land later without a deploy, tag or
        // cleanup. The PR stays open on GitHub; the next Ship starts from scratch.
        let _ = ghc(&["pr", "merge", head, "--disable-auto"]);
        Err("The pull request didn't merge within 30 minutes. Auto-merge was turned off and nothing was shipped; the pull request is still open on GitHub.".into())
    }
}

/// Run Ship for the chosen branches, in order.
#[tauri::command]
pub async fn ship_run(app: AppHandle, project_id: String, branches: Vec<String>) -> Result<ship::ShipOutcome, String> {
    let p = project(&app, &project_id)?;
    if branches.is_empty() {
        return Err("Pick at least one branch.".into());
    }
    if !shipping().lock().insert(p.id.clone()) {
        return Err("A Ship is already running for this project.".into());
    }
    let res = tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        let p = p.clone();
        move || run_ship(&app, &p, &branches)
    })
    .await
    .map_err(|e| e.to_string());
    shipping().lock().remove(&p.id);
    res?
}

fn run_ship(app: &AppHandle, p: &Project, branches: &[String]) -> Result<ship::ShipOutcome, String> {
    let c = cfg(app, &p.id).filter(|c| c.enabled).ok_or("GitHub sync isn't on for this project.")?;
    let conf = c.config.clone().ok_or("fleet.yml is missing.")?;
    let git = git_for(app, &p.root_path);
    let local: Vec<Session> = app.state::<Fleet>().store.lock().sessions.iter().filter(|s| s.project_id == p.id && !s.archived).cloned().collect();
    // 1. Refuse while a chosen session is working or mid-rebase.
    for b in branches {
        if let Some(s) = local.iter().find(|s| &s.branch == b) {
            if s.alive && s.state == SessionState::Working {
                return Err(format!("{} · {} is still working. Ship when it's done.", s.glyph, s.title));
            }
            if let Some(wt) = &s.worktree_path {
                if let Some(op) = git.at(wt).operation_in_progress() {
                    return Err(format!("{} · {} has a {op} in progress. Finish it first.", s.glyph, s.title));
                }
            }
        }
    }
    let files: HashMap<String, state_branch::SessionFile> = state_branch::read_sessions(&git, state_branch::REMOTE).into_iter().map(|(_, f)| (f.branch.clone(), f)).collect();
    let items: Vec<ShipBranch> = branches
        .iter()
        .map(|b| {
            let f = files.get(b);
            let s = local.iter().find(|s| &s.branch == b);
            ShipBranch {
                branch: b.clone(),
                title: s.map(|s| s.title.clone()).or_else(|| f.map(|f| f.title.clone())).unwrap_or_else(|| if b.contains("config-") { "Update fleet settings".into() } else { b.clone() }),
                glyph: s.map(|s| s.glyph.clone()).or_else(|| f.map(|f| f.glyph.clone())).unwrap_or_else(|| "—".into()),
            }
        })
        .collect();
    let pid = p.id.clone();
    let app2 = app.clone();
    let progress = move |pr: ship::Progress| {
        let _ = app2.emit("ship-progress", serde_json::json!({ "projectId": pid, "step": pr.step, "branch": pr.branch, "ok": pr.ok }));
    };
    let run = streaming_runner(app, &p.id);
    let pr = pr_merge(app, &p.id);
    let input = ship::ShipInput {
        git: &git,
        default_branch: c.default_branch.clone(),
        deploy_dir: PathBuf::from(&p.root_path).join(".fleet").join("deploy"),
        branches: items,
        prefix: conf.sync.branch_prefix.clone(),
        checks: conf.sync.checks.clone(),
        setup: conf.sync.setup.clone(),
        deploy_command: conf.sync.deploy_command.clone(),
        merge_style: conf.sync.merge_style.clone(),
        machine: machine_name(),
        run: &run,
        progress: &progress,
        via_pr: if conf.sync.main_is_protected { Some(&pr) } else { None },
        stamp: ship::ship_stamp(),
    };
    let out = ship::ship(&input);
    // main didn't move (offline, rejected, PR not merged): nothing was shipped, every branch
    // and session stays exactly as it was.
    if !out.pushed {
        return Ok(out);
    }
    // Sessions: shipped ones start fresh from the new main; skipped ones are told why.
    for (b, r) in &out.results {
        let Some(s) = local.iter().find(|s| &s.branch == b) else { continue };
        match r {
            BranchResult::Merged { .. } | BranchResult::Empty => load::detach_shipped(app, s, &c.default_branch),
            BranchResult::Conflict { files, .. } => super::fix::after_ship_conflict(app, s, &c.default_branch, conf.sync.merge_style == "rebase", files),
            BranchResult::ChecksFailed { check, output } => tell_session(app, s, &format!("Ship skipped this branch: the check `{check}` failed."), output),
        }
    }
    for b in &out.others {
        if let Some(s) = local.iter().find(|s| &s.branch == b) {
            let skipped = out.results.iter().any(|(x, r)| x == b && !matches!(r, BranchResult::Merged { .. } | BranchResult::Empty));
            if !skipped && app.state::<SyncState>().presence.lock().contains_key(&s.id) {
                let (app, id) = (app.clone(), s.id.clone());
                std::thread::spawn(move || load::rebase_after_ship(&app, &id));
            }
        }
    }
    // Shipped config branches: drop them locally too.
    for (b, r) in &out.results {
        if matches!(r, BranchResult::Merged { .. }) && (b.contains("/config-") || b.contains("/revert-")) {
            let _ = git.run(&["branch", "-q", "-D", b]);
        }
    }
    refresh_configs(app, Some(&p.id));
    if let Some(pp) = app.state::<Fleet>().store.lock().projects.iter().find(|x| x.id == p.id).cloned() {
        let app = app.clone();
        std::thread::spawn(move || fetch_project(&app, &pp));
    }
    let _ = app.emit("ship-done", serde_json::json!({ "projectId": p.id, "outcome": &out }));
    Ok(out)
}

/// A skipped branch: the session needs you, and the reason goes in as its next message.
fn tell_session(app: &AppHandle, s: &Session, what: &str, output: &str) {
    let msg = format!("{what} Please fix it on this branch, then tell me so I can ship it.\n\n```\n{}\n```", output.trim().chars().rev().take(4000).collect::<String>().chars().rev().collect::<String>());
    crate::update_session(app, &s.id, |x| {
        x.state = SessionState::NeedsReply;
        x.state_since = now_ms();
        x.pending_question = Some(what.to_string());
        true
    });
    if s.alive {
        let f = app.state::<Fleet>();
        let data = format!("\x1b[200~{}\x1b[201~", msg.replace("\x1b[201~", ""));
        let _ = f.ptys.write(&s.id, data.as_bytes());
        std::thread::sleep(Duration::from_millis(150));
        let _ = f.ptys.write(&s.id, b"\r");
    }
}

/// Rollback: run the deploy command at the previous deploy tag. main is never rewritten.
#[tauri::command]
pub async fn ship_redeploy(app: AppHandle, project_id: String, tag: Option<String>) -> Result<String, String> {
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let c = cfg(&app, &p.id).filter(|c| c.enabled).ok_or("GitHub sync isn't on.")?;
        let conf = c.config.clone().ok_or("fleet.yml is missing.")?;
        let cmd = conf.sync.deploy_command.clone().filter(|d| !d.trim().is_empty()).ok_or("fleet.yml has no deployCommand.")?;
        let git = git_for(&app, &p.root_path);
        let _ = git.run(&["fetch", "--prune", "--tags", "origin"]);
        let log = state_branch::read_deploys(&git, state_branch::REMOTE);
        let tag = match tag {
            Some(t) => t,
            None => ship::previous_deploy(&log).ok_or("There's no previous deploy to go back to.")?,
        };
        if git.rev(&format!("refs/tags/{tag}")).is_none() {
            return Err(format!("The tag {tag} isn't on GitHub or this Mac, so it can't be redeployed."));
        }
        if !shipping().lock().insert(p.id.clone()) {
            return Err("A Ship is already running for this project.".into());
        }
        let run = streaming_runner(&app, &p.id);
        let (code, _) = ship::redeploy(&git, &PathBuf::from(&p.root_path).join(".fleet").join("deploy"), &format!("refs/tags/{tag}"), conf.sync.setup.as_deref(), &cmd, &run);
        shipping().lock().remove(&p.id);
        let mut log = state_branch::read_deploys(&git, state_branch::REMOTE);
        log.push(state_branch::DeployEntry {
            tag: format!("{tag} (redeployed)"),
            commit: git.rev(&format!("refs/tags/{tag}")).unwrap_or_default(),
            branches: vec![],
            machine: machine_name(),
            at: state_branch::now_stamp(),
            result: if code == 0 { "redeployed".into() } else { "deploy_failed".into() },
        });
        if state_branch::write(&git, &[("deploys.yml".into(), Some(state_branch::to_yaml(&log)))], &format!("state: redeploy {tag}")).is_ok() {
            load::push_state(&git);
        }
        let _ = app.emit("ship-done", serde_json::json!({ "projectId": p.id }));
        if code == 0 { Ok(tag) } else { Err(format!("The deploy command failed (exit {code}) — see the log.")) }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The last Ship pushed main but its deploy failed or was skipped: run the deploy again at
/// GitHub's main (exactly what's shipped) and tag it like a normal deploy.
#[tauri::command]
pub async fn ship_retry_deploy(app: AppHandle, project_id: String) -> Result<String, String> {
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let c = cfg(&app, &p.id).filter(|c| c.enabled).ok_or("GitHub sync isn't on.")?;
        let conf = c.config.clone().ok_or("fleet.yml is missing.")?;
        let cmd = conf.sync.deploy_command.clone().filter(|d| !d.trim().is_empty()).ok_or("fleet.yml has no deployCommand.")?;
        let git = git_for(&app, &p.root_path);
        git.run(&["fetch", "--prune", "origin"]).map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
        let main = git.rev(&format!("refs/remotes/origin/{}", c.default_branch)).ok_or("GitHub has no main.")?;
        if !shipping().lock().insert(p.id.clone()) {
            return Err("A Ship is already running for this project.".into());
        }
        let run = streaming_runner(&app, &p.id);
        let (code, _) = ship::redeploy(&git, &PathBuf::from(&p.root_path).join(".fleet").join("deploy"), &main, conf.sync.setup.as_deref(), &cmd, &run);
        shipping().lock().remove(&p.id);
        let mut tag = format!("deploy-{}", ship::ship_stamp());
        if git.rev(&format!("refs/tags/{tag}")).is_some() {
            tag = format!("{tag}-{}", &main[..6]);
        }
        if code == 0 && git.run(&["tag", &tag, &main]).is_ok() {
            let r = format!("refs/tags/{tag}");
            let _ = pusher::push(&git, &[pusher::Refspec::new(&r, &r)], &[], None);
        }
        let mut log = state_branch::read_deploys(&git, state_branch::REMOTE);
        log.push(state_branch::DeployEntry {
            tag: if code == 0 { tag.clone() } else { format!("{tag}-failed") },
            commit: main.clone(),
            branches: vec![],
            machine: machine_name(),
            at: state_branch::now_stamp(),
            result: if code == 0 { "deployed".into() } else { "deploy_failed".into() },
        });
        if state_branch::write(&git, &[("deploys.yml".into(), Some(state_branch::to_yaml(&log)))], &format!("state: retry deploy {tag}")).is_ok() {
            load::push_state(&git);
        }
        let _ = app.emit("ship-done", serde_json::json!({ "projectId": p.id }));
        if code == 0 { Ok(tag) } else { Err(format!("The deploy command failed (exit {code}) — see the log.")) }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Features on main (one commit per shipped branch), newest first — for Undo.
#[tauri::command]
pub fn ship_history(app: AppHandle, project_id: String) -> Result<Vec<ShippedCommit>, String> {
    let p = project(&app, &project_id)?;
    let c = cfg(&app, &p.id).ok_or("unknown project")?;
    let git = Git::new(&p.root_path, None, None);
    let text = git.read(&["log", "-n", "40", "--format=%H%x1f%s%x1f%ct%x1f%(trailers:key=Fleet-Branch,valueonly,separator=)%x1e", &format!("refs/remotes/origin/{}", c.default_branch)]).unwrap_or_default();
    Ok(text
        .split('\x1e')
        .filter_map(|r| {
            let f: Vec<&str> = r.trim().split('\x1f').collect();
            (f.len() >= 4 && !f[3].trim().is_empty()).then(|| ShippedCommit { commit: f[0].into(), subject: f[1].into(), at: f[2].parse().unwrap_or(0), branch: f[3].trim().into() })
        })
        .collect())
}

/// Undo a shipped feature: a revert on a new branch; it goes out with the next Ship.
#[tauri::command]
pub async fn ship_undo(app: AppHandle, project_id: String, commit: String) -> Result<String, String> {
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let c = cfg(&app, &p.id).filter(|c| c.enabled).ok_or("GitHub sync isn't on.")?;
        let prefix = c.config.as_ref().map(|x| x.sync.branch_prefix.clone()).unwrap_or_else(|| "fleet/".into());
        let git = git_for(&app, &p.root_path);
        let _ = git.run(&["fetch", "--prune", "origin"]);
        let subject = git.read(&["log", "-1", "--format=%s", &commit]).map_err(|e| e.to_string())?.trim().to_string();
        let slug = crate::worktree::slugify(&subject, &|c| git.rev(&format!("refs/heads/{prefix}revert-{c}")).is_some() || git.rev(&format!("refs/remotes/origin/{prefix}revert-{c}")).is_some());
        let branch = format!("{prefix}revert-{slug}");
        ship::revert_branch(&git, &PathBuf::from(&p.root_path).join(".fleet").join("revert"), &c.default_branch, &commit, &branch)?;
        let f = state_branch::SessionFile {
            branch: branch.clone(),
            glyph: "↶".into(),
            color: "#8B949E".into(),
            title: format!("Undo: {subject}"),
            state: "done".into(),
            last_machine: machine_name(),
            updated_at: state_branch::now_stamp(),
            recap: vec![format!("Reverts {} ({subject})", &commit[..commit.len().min(8)])],
            ..Default::default()
        };
        let _ = state_branch::write(&git, &[(state_branch::file_for(&branch, &prefix), Some(state_branch::to_yaml(&f)))], &format!("state: undo {subject}"));
        match pusher::push(&git, &[pusher::Refspec::branch(&branch), pusher::Refspec::branch(state_branch::BRANCH)], &[], None) {
            pusher::PushOutcome::Pushed => Ok(branch),
            o => Err(format!("Couldn't push the undo branch: {o:?}")),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- cost guard

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowFinding {
    pub path: String,
    pub before: String,
    pub after: String,
}

/// The GitHub Actions event name (not a git push).
const PUSH_EVENT: &str = "push"; // workflow-event

/// Does this `on:` run on pushes to every branch?
pub fn push_is_broad(on: &serde_yaml_ng::Value) -> bool {
    use serde_yaml_ng::Value;
    match on {
        Value::String(s) => s == PUSH_EVENT,
        Value::Sequence(v) => v.iter().any(|x| x.as_str() == Some(PUSH_EVENT)),
        Value::Mapping(m) => match m.get(Value::String(PUSH_EVENT.into())) {
            None => false,
            Some(Value::Null) => true,
            Some(Value::Mapping(p)) => {
                let branches = p.get(Value::String("branches".into()));
                let ignore = p.get(Value::String("branches-ignore".into()));
                let tags_only = branches.is_none() && ignore.is_none() && p.get(Value::String("tags".into())).is_some();
                if tags_only {
                    return false;
                }
                if ignore.is_some() {
                    return true;
                }
                match branches {
                    None => true,
                    Some(Value::Sequence(bs)) => bs.iter().any(|b| matches!(b.as_str(), Some("**") | Some("*") | Some("fleet/**") | Some("fleet/*"))),
                    Some(Value::String(b)) => b == "**" || b == "*",
                    _ => false,
                }
            }
            Some(_) => true,
        },
        _ => false,
    }
}

/// The same workflow with `push` limited to main and ship/deploy tags.
pub fn limit_push(text: &str, main: &str) -> Option<String> {
    use serde_yaml_ng::{Mapping, Value};
    let mut doc: Value = serde_yaml_ng::from_str(text).ok()?;
    let root = doc.as_mapping_mut()?;
    let key = Value::String("on".into());
    let on = root.get(&key)?.clone();
    if !push_is_broad(&on) {
        return None;
    }
    let mut m = match on {
        Value::String(s) => {
            let mut m = Mapping::new();
            m.insert(Value::String(s), Value::Null);
            m
        }
        Value::Sequence(v) => v.into_iter().map(|x| (x, Value::Null)).collect(),
        Value::Mapping(m) => m,
        _ => return None,
    };
    let mut push = match m.get(Value::String(PUSH_EVENT.into())) {
        Some(Value::Mapping(p)) => p.clone(),
        _ => Mapping::new(),
    };
    push.remove(Value::String("branches-ignore".into()));
    push.insert(Value::String("branches".into()), Value::Sequence(vec![Value::String(main.into())]));
    push.insert(Value::String("tags".into()), Value::Sequence(vec![Value::String("deploy-*".into()), Value::String("ship-*".into())]));
    m.insert(Value::String(PUSH_EVENT.into()), Value::Mapping(push));
    root.insert(key, Value::Mapping(m));
    let body = serde_yaml_ng::to_string(&doc).ok()?;
    Some(format!("# Limited by fleet: CI runs on {main} and ship/deploy tags; fleet runs fleet.yml checks locally at Ship.\n{body}"))
}

fn findings(git: &Git, main_ref: &str, main: &str) -> Vec<WorkflowFinding> {
    git.read_tree_files(main_ref, ".github/workflows/")
        .into_iter()
        .filter(|(p, _)| p.ends_with(".yml") || p.ends_with(".yaml"))
        .filter_map(|(path, b)| {
            let before = String::from_utf8_lossy(&b).to_string();
            limit_push(&before, main).map(|after| WorkflowFinding { path, before, after })
        })
        .collect()
}

/// GitHub Actions that run on every branch push (fleet pushes after every prompt).
#[tauri::command]
pub fn sync_workflows(app: AppHandle, project_id: String) -> Result<Vec<WorkflowFinding>, String> {
    let p = project(&app, &project_id)?;
    let c = cfg(&app, &p.id).ok_or("unknown project")?;
    let git = Git::new(&p.root_path, None, None);
    let main_ref = if git.rev(&format!("refs/remotes/origin/{}", c.default_branch)).is_some() { format!("refs/remotes/origin/{}", c.default_branch) } else { "HEAD".into() };
    Ok(findings(&git, &main_ref, &c.default_branch))
}

/// Apply the limits (after the user saw the change) on the settings branch.
#[tauri::command]
pub async fn sync_limit_workflows(app: AppHandle, project_id: String) -> Result<String, String> {
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let c = cfg(&app, &p.id).ok_or("unknown project")?;
        let git = Git::new(&p.root_path, None, None);
        let main_ref = format!("refs/remotes/origin/{}", c.default_branch);
        let f = findings(&git, &main_ref, &c.default_branch);
        if f.is_empty() {
            return Err("Nothing to change.".into());
        }
        let files = f.into_iter().map(|x| (x.path, Some(x.after.into_bytes()))).collect();
        commit_settings_branch(&app, Path::new(&p.root_path), files, "ci: run GitHub Actions on main and ship tags only", &c)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_guard() {
        let broad = "name: CI\non: push\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps: [{run: echo}]\n";
        let after = limit_push(broad, "main").unwrap();
        let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&after).unwrap();
        assert!(!push_is_broad(&v["on"]), "{after}");
        assert_eq!(v["on"]["push"]["branches"][0].as_str(), Some("main"));
        assert_eq!(v["jobs"]["t"]["runs-on"].as_str(), Some("ubuntu-latest"));
        let pr_and_push = "on:\n  pull_request:\n  push:\n    branches: ['**']\n";
        let a2 = limit_push(pr_and_push, "main").unwrap();
        let v2: serde_yaml_ng::Value = serde_yaml_ng::from_str(&a2).unwrap();
        assert!(v2["on"].as_mapping().unwrap().contains_key(serde_yaml_ng::Value::String("pull_request".into())));
        assert!(limit_push("on:\n  push:\n    branches: [main]\n", "main").is_none(), "already limited");
        assert!(limit_push("on:\n  push:\n    tags: ['v*']\n", "main").is_none(), "tags only");
        assert!(limit_push("on: [pull_request]\n", "main").is_none());
        assert!(limit_push("on: [push, pull_request]\n", "main").is_some());
    }
}

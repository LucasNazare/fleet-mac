//! Runtime side of project processes: start, stop (INT → TERM → KILL on the whole
//! process group), restart, crash auto-restart with backoff, port polling, and
//! processes left running detached when fleet quits.
use crate::model::{now_ms, ProcessDef, Project};
use crate::processes::{self, ProcessRun, RunStatus};
use crate::{notify, shell_env, Fleet};
use portable_pty::CommandBuilder;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub const STOP_TERM_AFTER: Duration = Duration::from_secs(5);
pub const STOP_KILL_AFTER: Duration = Duration::from_secs(10);

pub fn emit_run(app: &AppHandle, run: &ProcessRun) {
    let _ = app.emit("process-updated", run);
}

fn pid_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn find_def(app: &AppHandle, project_id: &str, def_id: &str) -> Option<(Project, ProcessDef)> {
    let f = app.state::<Fleet>();
    let store = f.store.lock();
    let p = store.projects.iter().find(|p| p.id == project_id)?.clone();
    let d = p.processes.iter().find(|d| d.id == def_id)?.clone();
    Some((p, d))
}

/// KEY=VALUE lines of a .env file (quotes stripped, `export ` allowed).
pub fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.starts_with('#') || l.is_empty() {
                return None;
            }
            let l = l.strip_prefix("export ").unwrap_or(l);
            let (k, v) = l.split_once('=')?;
            let v = v.trim();
            let v = if (v.starts_with('"') && v.ends_with('"') || v.starts_with('\'') && v.ends_with('\'')) && v.len() >= 2 { &v[1..v.len() - 1] } else { v };
            Some((k.trim().to_string(), v.to_string()))
        })
        .collect()
}

/// Values for fleet.yml `env` names: the main checkout's .env files, then the worktree's.
fn dotenv_values(dirs: &[PathBuf], names: &[String]) -> Vec<(String, String)> {
    let mut found: std::collections::HashMap<String, String> = Default::default();
    for d in dirs {
        for f in [".env", ".env.local"] {
            if let Ok(t) = std::fs::read_to_string(d.join(f)) {
                for (k, v) in parse_dotenv(&t) {
                    found.insert(k, v);
                }
            }
        }
    }
    names.iter().filter_map(|n| found.get(n).map(|v| (n.clone(), v.clone()))).collect()
}

/// Start with the same base (session worktree) as the previous run of this process.
pub fn start(app: &AppHandle, project_id: &str, def_id: &str, cols: u16, rows: u16) -> Result<ProcessRun, String> {
    let prev = app.state::<Fleet>().runs.lock().get(&processes::key(project_id, def_id)).and_then(|r| r.session_id.clone());
    start_in(app, project_id, def_id, cols, rows, prev)
}

/// Start in a session's worktree (when it has one in this project), else the project root.
pub fn start_in(app: &AppHandle, project_id: &str, def_id: &str, cols: u16, rows: u16, session_id: Option<String>) -> Result<ProcessRun, String> {
    let (project, def) = find_def(app, project_id, def_id).ok_or("unknown process")?;
    let f = app.state::<Fleet>();
    let key = processes::key(project_id, def_id);
    if let Some(r) = f.runs.lock().get(&key) {
        if r.status == RunStatus::Running {
            return Ok(r.clone());
        }
    }
    let base_session = session_id.as_ref().and_then(|sid| {
        let store = f.store.lock();
        store.sessions.iter().find(|s| &s.id == sid && s.project_id == project_id).and_then(|s| s.worktree_path.clone().map(|w| (s.id.clone(), s.glyph.clone(), w)))
    });
    let base: PathBuf = base_session.as_ref().map(|b| PathBuf::from(&b.2)).filter(|p| p.is_dir()).unwrap_or_else(|| PathBuf::from(&project.root_path));
    let cwd: PathBuf = base.join(if def.cwd.is_empty() { "." } else { &def.cwd });
    if !cwd.is_dir() {
        return Err(format!("Working folder {} doesn't exist", cwd.display()));
    }
    let shell = shell_env::login_env().get("SHELL").cloned().unwrap_or_else(|| "/bin/zsh".into());
    let mut cmd = CommandBuilder::new(&shell);
    // SIGHUP is ignored so the process can outlive fleet if the user chooses
    // "leave running"; fleet itself always stops processes with INT/TERM/KILL.
    cmd.args(["-c", &format!("trap '' HUP; {}", def.command)]);
    cmd.cwd(&cwd);
    cmd.env_clear();
    for (k, v) in shell_env::login_env() {
        cmd.env(k, v);
    }
    for (k, v) in &def.env {
        cmd.env(k, v);
    }
    if !def.env_from.is_empty() {
        let root = PathBuf::from(&project.root_path);
        let sub = if def.cwd.is_empty() { "." } else { &def.cwd };
        let dirs = [root.clone(), root.join(sub), base.clone(), cwd.clone()];
        for (k, v) in dotenv_values(&dirs, &def.env_from) {
            cmd.env(k, v);
        }
    }
    cmd.env("FORCE_COLOR", "1");

    let (app2, key2) = (app.clone(), key.clone());
    let pid = f
        .ptys
        .spawn(&key, cmd, cols, rows, move |code, current| {
            if current {
                on_exit(&app2, &key2, code);
            }
        })
        .map_err(|e| e.to_string())?;

    let mut runs = f.runs.lock();
    let prev_restarts = runs.get(&key).map(|r| r.restarts).unwrap_or(0);
    let run = ProcessRun {
        key: key.clone(),
        project_id: project_id.into(),
        def_id: def_id.into(),
        pid: Some(pid),
        status: RunStatus::Running,
        exit_code: None,
        detected_ports: vec![],
        started_at: now_ms(),
        restarts: prev_restarts,
        next_restart_at: None,
        stopping: false,
        restart_after_stop: false,
        detached: false,
        session_id: base_session.as_ref().map(|b| b.0.clone()),
        base_label: base_session.as_ref().map(|b| b.1.clone()),
    };
    runs.insert(key, run.clone());
    emit_run(app, &run);
    Ok(run)
}

fn on_exit(app: &AppHandle, key: &str, code: Option<i32>) {
    let f = app.state::<Fleet>();
    let mut runs = f.runs.lock();
    let Some(run) = runs.get_mut(key) else { return };
    run.pid = None;
    run.exit_code = code;
    run.detected_ports.clear();
    let ran_for = now_ms() - run.started_at;
    if run.stopping {
        run.stopping = false;
        run.status = RunStatus::Stopped;
        run.restarts = 0;
        let restart = std::mem::take(&mut run.restart_after_stop);
        let r = run.clone();
        drop(runs);
        emit_run(app, &r);
        if restart {
            let (cols, rows) = (120, 32);
            if let Err(e) = start(app, &r.project_id, &r.def_id, cols, rows) {
                let _ = app.emit("toast", e);
            }
        }
        return;
    }
    if code == Some(0) {
        run.status = RunStatus::Exited;
        run.restarts = 0;
        let r = run.clone();
        drop(runs);
        emit_run(app, &r);
        return;
    }
    // Crash.
    run.status = RunStatus::Crashed;
    if ran_for > 60_000 {
        run.restarts = 0;
    }
    let r = run.clone();
    drop(runs);
    let Some((project, def)) = find_def(app, &r.project_id, &r.def_id) else { return };
    let visible = {
        let v = f.visible.lock();
        v.1 && v.0.contains(&r.key)
    };
    if !visible {
        notify::send(&r.key, &format!("fleet — {}", project.name), &format!("✕ {} crashed in {} (exit {})", def.name, project.name, code.unwrap_or(-1)));
    }
    if def.auto_restart {
        let delay = processes::backoff_secs(r.restarts);
        let at = now_ms() + delay as i64 * 1000;
        if let Some(run) = f.runs.lock().get_mut(&r.key) {
            run.next_restart_at = Some(at);
            emit_run(app, run);
        }
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(delay));
            let f = app.state::<Fleet>();
            let go = {
                let mut runs = f.runs.lock();
                match runs.get_mut(&r.key) {
                    Some(run) if run.status == RunStatus::Crashed && run.next_restart_at == Some(at) => {
                        run.restarts += 1;
                        run.next_restart_at = None;
                        true
                    }
                    _ => false,
                }
            };
            if go {
                let _ = start(&app, &r.project_id, &r.def_id, 120, 32);
            }
        });
    } else {
        emit_run(app, &r);
    }
}

/// SIGINT now, SIGTERM after 5 s, SIGKILL after 10 s — to the whole process group.
pub fn stop(app: &AppHandle, key: &str, restart_after: bool) {
    let f = app.state::<Fleet>();
    let (pid, detached) = {
        let mut runs = f.runs.lock();
        let Some(run) = runs.get_mut(key) else { return };
        run.next_restart_at = None;
        if run.status != RunStatus::Running {
            // Cancel a pending auto-restart and mark stopped.
            if run.status == RunStatus::Crashed {
                run.status = RunStatus::Stopped;
            }
            let r = run.clone();
            drop(runs);
            emit_run(app, &r);
            return;
        }
        run.stopping = true;
        run.restart_after_stop = restart_after;
        let r = run.clone();
        emit_run(app, &r);
        (r.pid, r.detached)
    };
    let Some(pid) = pid else { return };
    crate::pty::signal_group(pid, libc::SIGINT);
    let app = app.clone();
    let key = key.to_string();
    std::thread::spawn(move || {
        let still = |app: &AppHandle| {
            let f = app.state::<Fleet>();
            let same = f.runs.lock().get(&key).map(|r| r.pid == Some(pid) && r.stopping).unwrap_or(false);
            same && pid_alive(pid)
        };
        std::thread::sleep(STOP_TERM_AFTER);
        if still(&app) {
            crate::pty::signal_group(pid, libc::SIGTERM);
        }
        std::thread::sleep(STOP_KILL_AFTER - STOP_TERM_AFTER);
        if still(&app) {
            crate::pty::signal_group(pid, libc::SIGKILL);
        }
        // Detached processes have no PTY reader to report their exit.
        if detached {
            for _ in 0..20 {
                if !pid_alive(pid) {
                    on_exit(&app, &key, None);
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    });
}

/// Restart; `session_id` Some(..) moves it to that session's worktree ("" = project root).
pub fn restart(app: &AppHandle, project_id: &str, def_id: &str, cols: u16, rows: u16, session_id: Option<String>) -> Result<(), String> {
    let key = processes::key(project_id, def_id);
    let f = app.state::<Fleet>();
    if let Some(sid) = session_id.clone() {
        if let Some(r) = f.runs.lock().get_mut(&key) {
            r.session_id = Some(sid).filter(|s| !s.is_empty());
        }
    }
    let running = f.runs.lock().get(&key).map(|r| r.status == RunStatus::Running).unwrap_or(false);
    if running {
        stop(app, &key, true);
        Ok(())
    } else {
        let sid = match session_id {
            Some(s) => Some(s).filter(|s| !s.is_empty()),
            None => f.runs.lock().get(&key).and_then(|r| r.session_id.clone()),
        };
        start_in(app, project_id, def_id, cols, rows, sid).map(|_| ())
    }
}

pub fn running_count(fleet: &Fleet) -> usize {
    fleet.runs.lock().values().filter(|r| r.status == RunStatus::Running).count()
}

/// Every 5 s: listening ports for each running process tree; liveness of detached ones.
pub fn start_port_poller(app: AppHandle) {
    std::thread::Builder::new()
        .name("port-poller".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let f = app.state::<Fleet>();
            let running: Vec<(String, u32, bool)> = f
                .runs
                .lock()
                .values()
                .filter(|r| r.status == RunStatus::Running)
                .filter_map(|r| r.pid.map(|p| (r.key.clone(), p, r.detached)))
                .collect();
            if running.is_empty() {
                continue;
            }
            for (key, pid, detached) in &running {
                if *detached && !pid_alive(*pid) {
                    on_exit(&app, key, None);
                }
            }
            let roots: Vec<u32> = running.iter().map(|r| r.1).collect();
            let ports = processes::listening_ports(&roots);
            let mut runs = f.runs.lock();
            for (key, pid, _) in running {
                let p = ports.get(&pid).cloned().unwrap_or_default();
                if let Some(run) = runs.get_mut(&key) {
                    if run.pid == Some(pid) && run.detected_ports != p {
                        run.detected_ports = p;
                        emit_run(&app, run);
                    }
                }
            }
        })
        .expect("spawn port poller");
}

/// On launch: re-adopt processes left running by a previous fleet, if still alive.
pub fn adopt_detached(app: &AppHandle, detached: Vec<ProcessRun>) {
    let f = app.state::<Fleet>();
    let mut runs = f.runs.lock();
    for mut r in detached {
        if r.pid.map(pid_alive).unwrap_or(false) {
            r.status = RunStatus::Running;
            r.detached = true;
            r.stopping = false;
            runs.insert(r.key.clone(), r);
        }
    }
}

/// Auto-start processes marked for it (fleet launch = project open).
pub fn autostart(app: &AppHandle, project_ids: Option<&[String]>) {
    let projects = app.state::<Fleet>().store.lock().projects.clone();
    for p in projects {
        if project_ids.map(|ids| !ids.contains(&p.id)).unwrap_or(false) {
            continue;
        }
        for d in p.processes.iter().filter(|d| d.auto_start) {
            let already = app.state::<Fleet>().runs.lock().get(&processes::key(&p.id, &d.id)).map(|r| r.status == RunStatus::Running).unwrap_or(false);
            if !already {
                if let Err(e) = start(app, &p.id, &d.id, 120, 32) {
                    eprintln!("[fleet] autostart {} failed: {e}", d.name);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dotenv() {
        let v = super::parse_dotenv("# c\nDATABASE_URL=\"postgres://x\"\nexport A='1'\n\nB = two\n");
        assert_eq!(v, vec![("DATABASE_URL".into(), "postgres://x".into()), ("A".into(), "1".into()), ("B".into(), "two".into())]);
    }
}

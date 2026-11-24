//! SSH connections: one ControlMaster per connection (the user's own ssh, config,
//! keys, agent and known_hosts), status polling, and remote shells over it.
use crate::model::{now_ms, SshConnection};
use crate::{shell_env, Fleet};
use parking_lot::Mutex;
use portable_pty::CommandBuilder;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnStatus {
    pub conn_id: String,
    /// connecting | connected | disconnected | failed
    pub state: String,
    pub error: Option<String>,
    pub since: i64,
    /// Other project connections sharing the same server connection.
    pub shared_with: usize,
}

/// Live state of one physical server connection (shared by every project
/// connection that points at the same host, port, identity and options).
#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub state: String,
    pub error: Option<String>,
    pub since: i64,
}

#[derive(Default)]
pub struct SshState {
    /// server id → state of its one ControlMaster.
    pub servers: Mutex<HashMap<String, ServerStatus>>,
    /// Project connections the user has connected (each holds the shared master open).
    pub wanted: Mutex<std::collections::HashSet<String>>,
    /// Last status sent per project connection (to avoid duplicate events).
    pub sent: Mutex<HashMap<String, ConnStatus>>,
    /// Remote CLAUDE.md per project connection (folders differ), read on connect.
    pub claude_md: Mutex<HashMap<String, String>>,
}

/// POSIX single-quote a string for the remote shell.
pub fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Shell expression for a remote path, keeping a leading ~ expandable.
pub fn path_expr(p: &str) -> String {
    if p == "~" {
        "~".into()
    } else if let Some(rest) = p.strip_prefix("~/") {
        format!("~/{}", q(rest))
    } else {
        q(p)
    }
}

/// Identity of the physical server connection: same host, port, identity file and
/// options ⇒ same id ⇒ one shared ControlMaster, whatever project it's in.
pub fn server_id(c: &SshConnection) -> String {
    use std::hash::{Hash, Hasher};
    let norm = |s: Option<&str>| s.map(|x| x.trim().to_string()).unwrap_or_default();
    let key = format!("{}|{}|{}|{}", c.host.trim().to_lowercase(), c.port.map(|p| p.to_string()).unwrap_or_default(), norm(c.identity_file.as_deref()), norm(c.extra_args.as_deref()));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    format!("{:012x}", h.finish() & 0xffff_ffff_ffff)
}

/// Control socket of a server; kept short because socket paths are limited to 104 bytes.
pub fn ctl_path_for(server: &str) -> PathBuf {
    crate::persist::app_dir().join("ssh").join(format!("{server}.sock"))
}

pub fn ctl_path(c: &SshConnection) -> PathBuf {
    ctl_path_for(&server_id(c))
}

/// Options for the master connection (port, identity, keep-alives, user extras).
pub fn master_args(c: &SshConnection) -> Vec<String> {
    let mut a: Vec<String> = vec!["-M".into(), "-N".into(), "-S".into(), ctl_path(c).to_string_lossy().into(), "-o".into(), "ControlPersist=no".into(), "-o".into(), "ServerAliveInterval=30".into(), "-o".into(), "ServerAliveCountMax=3".into()];
    if let Some(p) = c.port {
        a.extend(["-p".into(), p.to_string()]);
    }
    if let Some(i) = c.identity_file.as_deref().filter(|s| !s.trim().is_empty()) {
        a.extend(["-i".into(), i.trim().to_string()]);
    }
    if let Some(x) = c.extra_args.as_deref() {
        a.extend(crate::settings::split_args(x));
    }
    a.push(c.host.clone());
    a
}

/// Args to run something over an existing master (never opens a second connection).
pub fn mux_args(c: &SshConnection) -> Vec<String> {
    vec!["-S".into(), ctl_path(c).to_string_lossy().into(), "-o".into(), "ControlMaster=no".into(), "-o".into(), "BatchMode=yes".into(), c.host.clone()]
}

fn ssh_bin() -> PathBuf {
    shell_env::which("ssh").unwrap_or_else(|| PathBuf::from("/usr/bin/ssh"))
}

pub fn find_conn(app: &AppHandle, conn_id: &str) -> Option<(String, SshConnection)> {
    let f = app.state::<Fleet>();
    let store = f.store.lock();
    store.projects.iter().find_map(|p| p.ssh_connections.iter().find(|c| c.id == conn_id).map(|c| (p.id.clone(), c.clone())))
}

fn all_conns(app: &AppHandle) -> Vec<SshConnection> {
    let f = app.state::<Fleet>();
    let store = f.store.lock();
    store.projects.iter().flat_map(|p| p.ssh_connections.iter().cloned()).collect()
}

/// Status a project connection shows: the shared server's state if this connection
/// is connected (wanted), otherwise disconnected.
fn conn_status(app: &AppHandle, c: &SshConnection, all: &[SshConnection]) -> ConnStatus {
    let st = app.state::<SshState>();
    let sid = server_id(c);
    let wanted = st.wanted.lock().contains(&c.id);
    let server = st.servers.lock().get(&sid).cloned();
    let shared_with = all.iter().filter(|o| o.id != c.id && server_id(o) == sid && st.wanted.lock().contains(&o.id)).count();
    match (wanted, server) {
        (true, Some(s)) => ConnStatus { conn_id: c.id.clone(), state: s.state, error: s.error, since: s.since, shared_with },
        _ => ConnStatus { conn_id: c.id.clone(), state: "disconnected".into(), error: None, since: now_ms(), shared_with },
    }
}

/// Push the status of every project connection on `server` to the UI (only changes).
fn emit_server(app: &AppHandle, server: &str) {
    let all = all_conns(app);
    for c in all.iter().filter(|c| server_id(c) == server) {
        let s = conn_status(app, c, &all);
        let st = app.state::<SshState>();
        let mut sent = st.sent.lock();
        let same = sent.get(&c.id).map(|p| p.state == s.state && p.error == s.error && p.shared_with == s.shared_with).unwrap_or(false);
        if !same {
            sent.insert(c.id.clone(), s.clone());
            let _ = app.emit("ssh-status", &s);
        }
    }
}

fn set_server(app: &AppHandle, server: &str, state: &str, error: Option<String>) {
    {
        let st = app.state::<SshState>();
        let mut m = st.servers.lock();
        if m.get(server).map(|s| s.state == state && s.error == error).unwrap_or(false) {
            return;
        }
        m.insert(server.into(), ServerStatus { state: state.into(), error, since: now_ms() });
    }
    emit_server(app, server);
}

pub fn is_connected(app: &AppHandle, conn_id: &str) -> bool {
    let Some((_, c)) = find_conn(app, conn_id) else { return false };
    let st = app.state::<SshState>();
    st.wanted.lock().contains(conn_id) && st.servers.lock().get(&server_id(&c)).map(|s| s.state == "connected").unwrap_or(false)
}

fn check(c: &SshConnection) -> bool {
    ctl_path(c).exists()
        && Command::new(ssh_bin()).args(["-S", &ctl_path(c).to_string_lossy(), "-O", "check", &c.host]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// Run a command on the server over the master; returns stdout.
pub fn run_remote(c: &SshConnection, script: &str) -> Result<String, String> {
    let out = Command::new(ssh_bin()).args(mux_args(c)).arg("--").arg(format!("bash -c {}", q(script))).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn load_claude_md(app: &AppHandle, c: SshConnection) {
    let app = app.clone();
    std::thread::spawn(move || {
        let md = run_remote(&c, &format!("cd {} && cat CLAUDE.md 2>/dev/null | head -c 40000", path_expr(&c.remote_path))).unwrap_or_default();
        app.state::<SshState>().claude_md.lock().insert(c.id.clone(), md);
    });
}

/// Connect a project's connection. Reuses the server's master if another project
/// already opened it; otherwise starts one in a PTY (`ssh:<server>`) so password /
/// 2FA / host-key prompts are visible.
#[tauri::command]
pub fn ssh_connect(app: AppHandle, conn_id: String, cols: u16, rows: u16) -> Result<(), String> {
    let (_, c) = find_conn(&app, &conn_id).ok_or("unknown connection")?;
    let sid = server_id(&c);
    app.state::<SshState>().wanted.lock().insert(conn_id.clone());
    let current = app.state::<SshState>().servers.lock().get(&sid).map(|s| s.state.clone());
    if current.as_deref() == Some("connecting") {
        emit_server(&app, &sid);
        return Ok(());
    }
    if check(&c) {
        set_server(&app, &sid, "connected", None);
        emit_server(&app, &sid);
        load_claude_md(&app, c);
        return Ok(());
    }
    let ctl = ctl_path(&c);
    std::fs::create_dir_all(ctl.parent().unwrap()).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&ctl);
    let mut cmd = CommandBuilder::new(ssh_bin());
    cmd.args(master_args(&c));
    cmd.env_clear();
    for (k, v) in shell_env::login_env() {
        cmd.env(k, v);
    }
    set_server(&app, &sid, "connecting", None);
    let (app2, sid2) = (app.clone(), sid.clone());
    app.state::<Fleet>()
        .ptys
        .spawn(&format!("ssh:{sid}"), cmd, cols, rows, move |code, current| {
            if !current {
                return;
            }
            let was = app2.state::<SshState>().servers.lock().get(&sid2).map(|s| s.state.clone()).unwrap_or_default();
            if was == "connecting" && code.unwrap_or(0) != 0 {
                set_server(&app2, &sid2, "failed", Some(format!("ssh exited with code {}", code.unwrap_or(-1))));
            } else {
                set_server(&app2, &sid2, "disconnected", None);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Disconnect a project's connection. The shared master only closes when no
/// other project still uses that server.
#[tauri::command]
pub fn ssh_disconnect(app: AppHandle, conn_id: String) -> Result<(), String> {
    let Some((_, c)) = find_conn(&app, &conn_id) else { return Ok(()) };
    let sid = server_id(&c);
    app.state::<SshState>().wanted.lock().remove(&conn_id);
    let all = all_conns(&app);
    let still_used = {
        let wanted = app.state::<SshState>().wanted.lock().clone();
        all.iter().any(|o| server_id(o) == sid && wanted.contains(&o.id))
    };
    if !still_used {
        let _ = Command::new(ssh_bin()).args(["-S", &ctl_path(&c).to_string_lossy(), "-O", "exit", &c.host]).stdin(Stdio::null()).output();
        app.state::<Fleet>().ptys.kill(&format!("ssh:{sid}"));
        set_server(&app, &sid, "disconnected", None);
    }
    emit_server(&app, &sid);
    Ok(())
}

/// PTY id of the master for a project connection (to show its prompt terminal).
#[tauri::command]
pub fn ssh_master_pty(app: AppHandle, conn_id: String) -> Option<String> {
    find_conn(&app, &conn_id).map(|(_, c)| format!("ssh:{}", server_id(&c)))
}

#[tauri::command]
pub fn ssh_statuses(app: AppHandle) -> Vec<ConnStatus> {
    let all = all_conns(&app);
    all.iter().map(|c| conn_status(&app, c, &all)).collect()
}

/// Every 3 s: is each shared master still up? On first connect, read the CLAUDE.md
/// of every project connection using it.
pub fn start_poller(app: AppHandle) {
    std::thread::Builder::new()
        .name("ssh-poller".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(3));
            let watching: Vec<(String, String)> = app.state::<SshState>().servers.lock().iter().filter(|(_, s)| s.state == "connecting" || s.state == "connected").map(|(k, s)| (k.clone(), s.state.clone())).collect();
            if watching.is_empty() {
                continue;
            }
            let all = all_conns(&app);
            for (sid, state) in watching {
                let Some(c) = all.iter().find(|c| server_id(c) == sid) else { continue };
                let up = check(c);
                if up && state != "connected" {
                    set_server(&app, &sid, "connected", None);
                    let wanted = app.state::<SshState>().wanted.lock().clone();
                    for c in all.iter().filter(|c| server_id(c) == sid && wanted.contains(&c.id)) {
                        load_claude_md(&app, c.clone());
                    }
                } else if !up && state == "connected" {
                    set_server(&app, &sid, "disconnected", Some("The connection dropped.".into()));
                }
            }
        })
        .expect("ssh poller");
}

/// A login shell on the server, in the connection's folder, over the master.
pub fn shell_command(c: &SshConnection) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(ssh_bin());
    cmd.args(["-tt"]);
    cmd.args(mux_args(c));
    cmd.arg("--");
    cmd.arg(format!("cd {} 2>/dev/null; exec \"${{SHELL:-/bin/bash}}\" -l", path_expr(&c.remote_path)));
    cmd.env_clear();
    for (k, v) in shell_env::login_env() {
        cmd.env(k, v);
    }
    cmd
}

/// Save (create/update) a connection on a project.
#[tauri::command]
pub fn ssh_save(app: AppHandle, project_id: String, mut conn: SshConnection) -> Result<SshConnection, String> {
    if conn.host.trim().is_empty() {
        return Err("Host is required (an ssh-config alias or user@host).".into());
    }
    if conn.id.is_empty() {
        conn.id = uuid::Uuid::new_v4().to_string();
    }
    if conn.name.trim().is_empty() {
        conn.name = conn.host.clone();
    }
    if conn.remote_path.trim().is_empty() {
        conn.remote_path = "~".into();
    }
    let f = app.state::<Fleet>();
    let mut store = f.store.lock();
    let p = store.projects.iter_mut().find(|p| p.id == project_id).ok_or("unknown project")?;
    match p.ssh_connections.iter_mut().find(|c| c.id == conn.id) {
        Some(c) => *c = conn.clone(),
        None => p.ssh_connections.push(conn.clone()),
    }
    let _ = app.emit("projects-changed", &store.projects);
    drop(store);
    f.mark_dirty();
    emit_server(&app, &server_id(&conn));
    Ok(conn)
}

#[tauri::command]
pub fn ssh_delete(app: AppHandle, project_id: String, conn_id: String) -> Result<(), String> {
    let _ = ssh_disconnect(app.clone(), conn_id.clone());
    let f = app.state::<Fleet>();
    let mut store = f.store.lock();
    if let Some(p) = store.projects.iter_mut().find(|p| p.id == project_id) {
        p.ssh_connections.retain(|c| c.id != conn_id);
    }
    let _ = app.emit("projects-changed", &store.projects);
    drop(store);
    f.mark_dirty();
    Ok(())
}

/// MCP config handing claude the remote tools for this connection.
pub fn write_mcp_config(c: &SshConnection, mcp_bin: &std::path::Path) -> Result<PathBuf, String> {
    let dir = crate::persist::app_dir().join("remote");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.mcp.json", c.id));
    let cfg = serde_json::json!({
        "mcpServers": {
            "fleet_remote": {
                "command": mcp_bin.to_string_lossy(),
                "args": ["--ctl", ctl_path(c).to_string_lossy(), "--host", c.host, "--root", c.remote_path, "--name", c.name],
            }
        }
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&cfg).unwrap()).map_err(|e| e.to_string())?;
    Ok(path)
}

pub const LOCAL_TOOLS_OFF: &[&str] = &["Bash", "Read", "Edit", "Write", "MultiEdit", "Glob", "Grep", "NotebookEdit", "LS"];

pub fn system_prompt(c: &SshConnection, claude_md: &str) -> String {
    let mut s = format!(
        "You are working on the remote server `{}` (fleet connection \"{}\"), in the folder `{}`. \
         The code lives on that server, not on this machine. Use the mcp__fleet_remote__* tools for every file and shell operation \
         (bash, read, write, edit, glob, grep, ls); relative paths resolve against that folder. Local file and shell tools are disabled.",
        c.host, c.name, c.remote_path
    );
    if !claude_md.trim().is_empty() {
        s.push_str("\n\nThe project's CLAUDE.md on the server:\n\n");
        s.push_str(claude_md);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(q("it's"), "'it'\\''s'");
        assert_eq!(path_expr("~/apps/my app"), "~/'apps/my app'");
        assert_eq!(path_expr("/srv/x"), "'/srv/x'");
        assert_eq!(path_expr("~"), "~");
    }

    fn conn(id: &str, host: &str, path: &str) -> SshConnection {
        SshConnection { id: id.into(), name: id.into(), host: host.into(), remote_path: path.into(), ..Default::default() }
    }

    #[test]
    fn same_server_shares_one_connection() {
        let a = conn("a", "deploy@prod", "~/api");
        let b = conn("b", "Deploy@Prod ", "~/web");
        let c = SshConnection { port: Some(2222), ..conn("c", "deploy@prod", "~/api") };
        assert_eq!(server_id(&a), server_id(&b), "same host, different projects/folders → one master");
        assert_eq!(ctl_path(&a), ctl_path(&b));
        assert_ne!(server_id(&a), server_id(&c), "different port → different server");
    }

    #[test]
    fn socket_path_fits() {
        let p = ctl_path(&conn("x", "deploy@a-very-long-hostname.example.com", "~"));
        assert!(p.to_string_lossy().len() < 104, "{}", p.display());
    }

    #[test]
    fn master_args_built() {
        let c = SshConnection { id: "abc".into(), name: "prod".into(), host: "deploy@prod".into(), port: Some(2222), identity_file: Some("~/.ssh/k".into()), remote_path: "~/app".into(), extra_args: Some("-J bastion".into()), auto_connect: false };
        let a = master_args(&c);
        assert_eq!(&a[..2], &["-M", "-N"]);
        assert!(a.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(a.windows(2).any(|w| w == ["-J", "bastion"]));
        assert_eq!(a.last().unwrap(), "deploy@prod");
        assert!(!a.iter().any(|x| x.contains("StrictHostKeyChecking")));
    }
}

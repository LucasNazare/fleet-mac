//! Language servers over stdio, bridged to Monaco's LSP client through a Tauri
//! channel. One server per (project root, language).
use crate::shell_env;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

type R<T> = Result<T, String>;

struct Server {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    root: String,
    ts_path: Option<String>,
}

#[derive(Default)]
pub struct LspState {
    servers: Mutex<HashMap<u32, Arc<Server>>>,
    next: AtomicU32,
}

/// Bundled resource (in the .app) or the repo's node_modules (dev).
fn resource(app: &AppHandle, rel: &str, dev_rel: &str) -> Option<PathBuf> {
    if let Ok(dir) = app.path().resource_dir() {
        let p = dir.join(rel);
        if p.exists() {
            return Some(p);
        }
    }
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(dev_rel);
    p.exists().then_some(p)
}

/// Command line for a language's server: user setting first, then built-ins.
fn server_command(app: &AppHandle, language: &str, custom: Option<Vec<String>>, root: &Path) -> R<(Vec<String>, Option<String>)> {
    if let Some(c) = custom.filter(|c| !c.is_empty()) {
        return Ok((c, None));
    }
    match language {
        "typescript" | "javascript" => {
            let cli = resource(app, "lsp/tsls/lib/cli.mjs", "node_modules/typescript-language-server/lib/cli.mjs").ok_or("typescript-language-server is missing from this build")?;
            let node = shell_env::which("node").ok_or("Node.js not found on your PATH (needed for TypeScript language features)")?;
            // Prefer the project's own TypeScript; fall back to the bundled one.
            let project_ts = root.join("node_modules/typescript/lib");
            let ts = if project_ts.join("tsserver.js").exists() { Some(project_ts) } else { resource(app, "lsp/typescript/lib", "node_modules/typescript/lib") };
            Ok((vec![node.to_string_lossy().into(), cli.to_string_lossy().into(), "--stdio".into()], ts.map(|p| p.to_string_lossy().into())))
        }
        "python" => {
            let p = shell_env::which("pyright-langserver").ok_or("pyright-langserver not found (npm i -g pyright)")?;
            Ok((vec![p.to_string_lossy().into(), "--stdio".into()], None))
        }
        "go" => Ok((vec![shell_env::which("gopls").ok_or("gopls not found")?.to_string_lossy().into()], None)),
        "rust" => Ok((vec![shell_env::which("rust-analyzer").ok_or("rust-analyzer not found")?.to_string_lossy().into()], None)),
        other => Err(format!("No language server configured for {other}")),
    }
}

fn write_frame(w: &mut impl Write, body: &str) -> std::io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    w.flush()
}

/// Read one `Content-Length` framed message.
pub fn read_frame(r: &mut impl BufRead) -> Option<String> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut buf = vec![0u8; len?];
    r.read_exact(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn file_uri(p: &str) -> String {
    format!("file://{}", p.split('/').map(|seg| seg.replace('%', "%25").replace(' ', "%20").replace('#', "%23").replace('?', "%3F")).collect::<Vec<_>>().join("/"))
}

/// Monaco's client sends `rootUri: null`; give the server the project instead.
pub fn patch_initialize(msg: &mut Value, root: &str, ts_path: Option<&str>) {
    if msg.get("method").and_then(|m| m.as_str()) != Some("initialize") {
        return;
    }
    let uri = file_uri(root);
    let name = Path::new(root).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let p = &mut msg["params"];
    p["rootUri"] = Value::String(uri.clone());
    p["rootPath"] = Value::String(root.into());
    p["processId"] = Value::from(std::process::id());
    p["workspaceFolders"] = serde_json::json!([{ "uri": uri, "name": name }]);
    if let Some(ts) = ts_path {
        p["initializationOptions"]["tsserver"]["path"] = Value::String(ts.into());
    }
}

#[tauri::command]
pub fn lsp_start(app: AppHandle, root: String, language: String, command: Option<Vec<String>>, on_message: Channel<String>, state: State<LspState>) -> R<u32> {
    let (argv, ts_path) = server_command(&app, &language, command, Path::new(&root))?;
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&root)
        .env_clear()
        .envs(shell_env::login_env())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", argv[0]))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let id = state.next.fetch_add(1, Ordering::Relaxed) + 1;
    state.servers.lock().insert(id, Arc::new(Server { stdin: Mutex::new(stdin), child: Mutex::new(child), root: root.clone(), ts_path }));
    std::thread::Builder::new().name(format!("lsp-{id}")).spawn(move || {
        let mut r = BufReader::new(stdout);
        while let Some(body) = read_frame(&mut r) {
            if on_message.send(body).is_err() {
                break;
            }
        }
    }).map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if cfg!(debug_assertions) {
                eprintln!("[lsp {id}] {line}");
            }
        }
    });
    Ok(id)
}

#[tauri::command]
pub fn lsp_send(id: u32, message: String, state: State<LspState>) -> R<()> {
    let srv = state.servers.lock().get(&id).cloned().ok_or("language server stopped")?;
    let mut v: Value = serde_json::from_str(&message).map_err(|e| e.to_string())?;
    patch_initialize(&mut v, &srv.root, srv.ts_path.as_deref());
    let body = v.to_string();
    let mut w = srv.stdin.lock();
    write_frame(&mut *w, &body).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn lsp_stop(id: u32, state: State<LspState>) {
    if let Some(s) = state.servers.lock().remove(&id) {
        let _ = s.child.lock().kill();
    }
}

pub fn stop_all(state: &LspState) {
    for (_, s) in state.servers.lock().drain() {
        let _ = s.child.lock().kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip() {
        let mut buf = vec![];
        write_frame(&mut buf, r#"{"a":"é"}"#).unwrap();
        write_frame(&mut buf, "{}").unwrap();
        let mut r = BufReader::new(&buf[..]);
        assert_eq!(read_frame(&mut r).unwrap(), r#"{"a":"é"}"#);
        assert_eq!(read_frame(&mut r).unwrap(), "{}");
        assert!(read_frame(&mut r).is_none());
    }

    #[test]
    fn initialize_gets_root() {
        let mut v = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":null,"capabilities":{}}});
        patch_initialize(&mut v, "/Users/me/my app", Some("/ts/lib"));
        assert_eq!(v["params"]["rootUri"], "file:///Users/me/my%20app");
        assert_eq!(v["params"]["workspaceFolders"][0]["name"], "my app");
        assert_eq!(v["params"]["initializationOptions"]["tsserver"]["path"], "/ts/lib");
        let mut other = serde_json::json!({"method":"textDocument/hover"});
        patch_initialize(&mut other, "/r", None);
        assert!(other.get("params").is_none());
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use std::io::BufReader;

    /// Real typescript-language-server: go to definition across files.
    #[test]
    fn ts_definition_across_files() {
        let Some(node) = shell_env::which("node") else { return };
        let cli = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../node_modules/typescript-language-server/lib/cli.mjs");
        let ts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../node_modules/typescript/lib");
        if !cli.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("fleet-lsp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ts"), "export function hello(n: number) {\n  return n + 1;\n}\n").unwrap();
        let b = "import { hello } from './a';\nconst x: number = hello(2);\n";
        std::fs::write(dir.join("b.ts"), b).unwrap();
        std::fs::write(dir.join("tsconfig.json"), "{\"compilerOptions\":{\"strict\":true}}").unwrap();
        let root = dir.canonicalize().unwrap().to_string_lossy().to_string();

        let mut child = Command::new(node).arg(&cli).arg("--stdio").current_dir(&root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut send = |v: Value| {
            let mut v = v;
            patch_initialize(&mut v, &root, Some(&ts.to_string_lossy()));
            write_frame(&mut stdin, &v.to_string()).unwrap();
        };
        let mut wait_id = |id: i64| -> Value {
            loop {
                let msg: Value = serde_json::from_str(&read_frame(&mut out).expect("server closed")).unwrap();
                if msg.get("id").and_then(|i| i.as_i64()) == Some(id) && msg.get("method").is_none() {
                    return msg;
                }
            }
        };
        send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}));
        let init = wait_id(1);
        assert!(init["result"]["capabilities"]["definitionProvider"].as_bool().unwrap_or(true));
        send(serde_json::json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
        let b_uri = file_uri(&format!("{root}/b.ts"));
        send(serde_json::json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":b_uri,"languageId":"typescript","version":1,"text":b}}}));
        // `hello` in `hello(2)` on line 1 (0-based), character 19. The project
        // loads asynchronously, so ask until the answer leaves b.ts.
        let mut s = String::new();
        for id in 2..8 {
            send(serde_json::json!({"jsonrpc":"2.0","id":id,"method":"textDocument/definition","params":{"textDocument":{"uri":b_uri},"position":{"line":1,"character":19}}}));
            s = wait_id(id)["result"].to_string();
            if s.contains("a.ts") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(700));
        }
        let _ = child.kill();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(s.contains("a.ts"), "definition result: {s}");
    }
}

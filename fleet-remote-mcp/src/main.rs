//! fleet-remote-mcp: Claude Code runs locally; these tools run on the server.
//! Every call goes through fleet's shared SSH ControlMaster (`ssh -S <ctl>`), so
//! there is no handshake per call and nothing is installed on the server.
//!
//! Usage: fleet-remote-mcp --ctl <socket> --host <host> --root <remote folder> [--name <label>]
//!        fleet-remote-mcp --local --root <folder>   (runs the same scripts locally; for tests)
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAX_OUT: usize = 30_000;
const MAX_FILE: usize = 4 * 1024 * 1024;

pub struct Remote {
    ctl: Option<String>,
    host: String,
    pub root: String,
    name: String,
}

pub struct Out {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub code: Option<i32>,
    pub timed_out: bool,
}

pub fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Shell expression for `path` (relative to root unless absolute), keeping ~ expandable.
pub fn path_expr(root: &str, path: &str) -> String {
    let p = if path.is_empty() || path == "." {
        root.to_string()
    } else if path.starts_with('/') || path.starts_with('~') {
        path.to_string()
    } else {
        format!("{}/{}", root.trim_end_matches('/'), path)
    };
    if p == "~" {
        "~".into()
    } else if let Some(rest) = p.strip_prefix("~/") {
        format!("~/{}", q(rest))
    } else {
        q(&p)
    }
}

impl Remote {
    fn connected(&self) -> bool {
        match &self.ctl {
            None => true,
            Some(ctl) => std::path::Path::new(ctl).exists(),
        }
    }

    /// Run a bash script on the server (or locally in --local mode).
    pub fn run(&self, script: &str, login: bool, stdin: Option<&[u8]>, timeout: Duration) -> Result<Out, String> {
        if !self.connected() {
            return Err(format!("Not connected to {} — reconnect it in fleet.", self.name));
        }
        let bash = if login { "bash -lc" } else { "bash -c" };
        let mut cmd = match &self.ctl {
            Some(ctl) => {
                let mut c = Command::new("ssh");
                c.args(["-S", ctl, "-o", "ControlMaster=no", "-o", "BatchMode=yes", &self.host, "--"]).arg(format!("{bash} {}", q(script)));
                c
            }
            None => {
                let mut c = Command::new("bash");
                c.arg(if login { "-lc" } else { "-c" }).arg(script);
                c
            }
        };
        // Own process group, so a timeout kills everything the command started.
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
        if let Some(data) = stdin {
            let mut w = child.stdin.take().unwrap();
            w.write_all(data).map_err(|e| e.to_string())?;
        }
        let mut so = child.stdout.take().unwrap();
        let mut se = child.stderr.take().unwrap();
        let t_out = std::thread::spawn(move || {
            let mut b = vec![];
            let _ = so.read_to_end(&mut b);
            b
        });
        let t_err = std::thread::spawn(move || {
            let mut b = vec![];
            let _ = se.read_to_end(&mut b);
            b
        });
        let t0 = Instant::now();
        let mut timed_out = false;
        let status = loop {
            if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                break Some(s);
            }
            if t0.elapsed() > timeout {
                timed_out = true;
                let _ = Command::new("kill").args(["-9", &format!("-{}", child.id())]).status();
                let _ = child.kill();
                break child.wait().ok();
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let stdout = t_out.join().unwrap_or_default();
        let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
        let code = status.and_then(|s| s.code());
        // ssh exits 255 when the connection itself fails.
        if self.ctl.is_some() && code == Some(255) && stderr.contains("ontrol") {
            return Err(format!("Not connected to {} — reconnect it in fleet. ({})", self.name, stderr.trim()));
        }
        Ok(Out { stdout, stderr, code, timed_out })
    }
}

fn clip(s: &str) -> String {
    if s.len() <= MAX_OUT {
        return s.to_string();
    }
    let mut cut = MAX_OUT;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n… (output truncated, {} more bytes)", &s[..cut], s.len() - cut)
}

fn str_arg<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    let v = a.get(k).and_then(|v| v.as_str());
    // Claude often reaches for its built-in tools' names (file_path); accept them too.
    if v.is_none() && k == "path" {
        return a.get("file_path").and_then(|v| v.as_str());
    }
    v
}

pub fn read_file(r: &Remote, path: &str) -> Result<String, String> {
    let o = r.run(&format!("head -c {} -- {}", MAX_FILE + 1, path_expr(&r.root, path)), false, None, Duration::from_secs(60))?;
    if o.code != Some(0) {
        return Err(o.stderr.trim().to_string().replace("head: ", ""));
    }
    if o.stdout.len() > MAX_FILE {
        return Err(format!("{path} is larger than 4 MB"));
    }
    if o.stdout.iter().take(8000).any(|b| *b == 0) {
        return Err(format!("{path} is a binary file"));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

pub fn write_file(r: &Remote, path: &str, content: &str) -> Result<(), String> {
    let p = path_expr(&r.root, path);
    let script = format!(
        "f={p}; d=$(dirname -- \"$f\"); mkdir -p -- \"$d\" && t=\"$f.fleet-tmp-$$\" && cat > \"$t\" && {{ [ -e \"$f\" ] && chmod --reference=\"$f\" \"$t\" 2>/dev/null; mv -f -- \"$t\" \"$f\"; }}"
    );
    let o = r.run(&script, false, Some(content.as_bytes()), Duration::from_secs(120))?;
    if o.code == Some(0) {
        Ok(())
    } else {
        Err(o.stderr.trim().to_string())
    }
}

/// Exact-match replace, same rules as Claude Code's Edit tool.
pub fn apply_edit(text: &str, old: &str, new: &str, all: bool) -> Result<(String, usize), String> {
    if old.is_empty() {
        return Err("old_string must not be empty".into());
    }
    if old == new {
        return Err("old_string and new_string are the same".into());
    }
    let n = text.matches(old).count();
    match (n, all) {
        (0, _) => Err("old_string was not found in the file".into()),
        (n, false) if n > 1 => Err(format!("old_string appears {n} times; add more context to make it unique, or set replace_all")),
        (n, true) => Ok((text.replace(old, new), n)),
        _ => Ok((text.replacen(old, new, 1), 1)),
    }
}

pub fn call_tool(r: &Remote, name: &str, a: &Value) -> Result<String, String> {
    match name {
        "bash" => {
            let command = str_arg(a, "command").ok_or("command is required")?;
            let ms = a.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(120_000).clamp(1_000, 600_000);
            let o = r.run(&format!("cd {} && {}", path_expr(&r.root, "."), command), true, None, Duration::from_millis(ms))?;
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            if !o.stderr.trim().is_empty() {
                if !s.is_empty() && !s.ends_with('\n') {
                    s.push('\n');
                }
                s.push_str(&o.stderr);
            }
            if o.timed_out {
                s.push_str(&format!("\n(timed out after {} ms)", ms));
            } else if o.code != Some(0) {
                s.push_str(&format!("\n(exit code {})", o.code.unwrap_or(-1)));
            }
            Ok(clip(if s.trim().is_empty() { "(no output)" } else { &s }))
        }
        "read" => {
            let path = str_arg(a, "path").ok_or("path is required")?;
            let text = read_file(r, path)?;
            let offset = a.get("offset").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
            let limit = a.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;
            let lines: Vec<&str> = text.lines().collect();
            if lines.is_empty() {
                return Ok("(empty file)".into());
            }
            let out: Vec<String> = lines
                .iter()
                .enumerate()
                .skip(offset - 1)
                .take(limit)
                .map(|(i, l)| {
                    let l = if l.chars().count() > 2000 { l.chars().take(2000).collect::<String>() + "…" } else { l.to_string() };
                    format!("{:>6}\t{}", i + 1, l)
                })
                .collect();
            let mut s = out.join("\n");
            if offset - 1 + limit < lines.len() {
                s.push_str(&format!("\n… ({} more lines; use offset/limit)", lines.len() - (offset - 1 + limit)));
            }
            Ok(s)
        }
        "write" => {
            let path = str_arg(a, "path").ok_or("path is required")?;
            let content = str_arg(a, "content").ok_or("content is required")?;
            write_file(r, path, content)?;
            Ok(format!("Wrote {} ({} bytes)", path, content.len()))
        }
        "edit" => {
            let path = str_arg(a, "path").ok_or("path is required")?;
            let old = str_arg(a, "old_string").ok_or("old_string is required")?;
            let new = str_arg(a, "new_string").ok_or("new_string is required")?;
            let all = a.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
            let text = read_file(r, path)?;
            let (updated, n) = apply_edit(&text, old, new, all)?;
            write_file(r, path, &updated)?;
            Ok(format!("Edited {path} ({n} replacement{})", if n == 1 { "" } else { "s" }))
        }
        "glob" => {
            let pattern = str_arg(a, "pattern").ok_or("pattern is required")?;
            let base = path_expr(&r.root, str_arg(a, "path").unwrap_or("."));
            let script = format!("cd {base} && shopt -s globstar nullglob dotglob 2>/dev/null; for f in {pattern}; do printf '%s\\n' \"$f\"; done | grep -v '^\\.git/' | head -n 1000");
            let o = r.run(&script, false, None, Duration::from_secs(60))?;
            let s = String::from_utf8_lossy(&o.stdout).into_owned();
            Ok(if s.trim().is_empty() { "(no files match)".into() } else { clip(&s) })
        }
        "grep" => {
            let pattern = str_arg(a, "pattern").ok_or("pattern is required")?;
            let path = path_expr(&r.root, str_arg(a, "path").unwrap_or("."));
            let i = a.get("ignore_case").and_then(|v| v.as_bool()).unwrap_or(false);
            let glob = str_arg(a, "glob").filter(|g| !g.is_empty());
            let rg = format!("rg -n --no-heading --color never {} {} -e {} -- {path}", if i { "-i" } else { "" }, glob.map(|g| format!("-g {}", q(g))).unwrap_or_default(), q(pattern));
            let gr = format!("grep -rnIE --exclude-dir=.git {} {} -e {} -- {path}", if i { "-i" } else { "" }, glob.map(|g| format!("--include={}", q(g))).unwrap_or_default(), q(pattern));
            let script = format!("if command -v rg >/dev/null 2>&1; then {rg}; else {gr}; fi | head -n 500");
            let o = r.run(&script, false, None, Duration::from_secs(120))?;
            let s = String::from_utf8_lossy(&o.stdout).into_owned();
            Ok(if s.trim().is_empty() { "(no matches)".into() } else { clip(&s) })
        }
        "ls" => {
            let o = r.run(&format!("ls -la -- {}", path_expr(&r.root, str_arg(a, "path").unwrap_or("."))), false, None, Duration::from_secs(30))?;
            if o.code != Some(0) {
                return Err(o.stderr.trim().to_string());
            }
            Ok(clip(&String::from_utf8_lossy(&o.stdout)))
        }
        other => Err(format!("unknown tool {other}")),
    }
}

pub fn tools(r: &Remote) -> Value {
    let on = format!("on {} ({})", r.name, r.host);
    let s = |props: Value, req: &[&str]| json!({ "type": "object", "properties": props, "required": req });
    json!([
        { "name": "bash", "description": format!("Run a shell command {on}, in {}. Use for git, tests, builds, logs, anything you'd run in a terminal there.", r.root),
          "inputSchema": s(json!({ "command": {"type":"string"}, "timeout_ms": {"type":"integer","description":"default 120000, max 600000"} }), &["command"]) },
        { "name": "read", "description": format!("Read a text file {on}. Paths are relative to {} unless absolute. Returns numbered lines.", r.root),
          "inputSchema": s(json!({ "path": {"type":"string"}, "offset": {"type":"integer","description":"1-based first line"}, "limit": {"type":"integer"} }), &["path"]) },
        { "name": "write", "description": format!("Create or overwrite a file {on} with the given content."),
          "inputSchema": s(json!({ "path": {"type":"string"}, "content": {"type":"string"} }), &["path", "content"]) },
        { "name": "edit", "description": format!("Replace an exact string in a file {on}. old_string must match exactly once unless replace_all is true. Read the file first."),
          "inputSchema": s(json!({ "path": {"type":"string"}, "old_string": {"type":"string"}, "new_string": {"type":"string"}, "replace_all": {"type":"boolean"} }), &["path", "old_string", "new_string"]) },
        { "name": "glob", "description": format!("List files {on} matching a bash glob (** supported), e.g. src/**/*.ts."),
          "inputSchema": s(json!({ "pattern": {"type":"string"}, "path": {"type":"string"} }), &["pattern"]) },
        { "name": "grep", "description": format!("Search file contents {on} (ripgrep if installed, else grep -E). Returns path:line:text."),
          "inputSchema": s(json!({ "pattern": {"type":"string"}, "path": {"type":"string"}, "glob": {"type":"string"}, "ignore_case": {"type":"boolean"} }), &["pattern"]) },
        { "name": "ls", "description": format!("List a directory {on}."),
          "inputSchema": s(json!({ "path": {"type":"string"} }), &[]) },
    ])
}

/// Handle one JSON-RPC message; None for notifications.
pub fn handle(r: &Remote, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let result = match method {
        "initialize" => json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "fleet-remote", "version": env!("CARGO_PKG_VERSION") },
            "instructions": format!("Tools that act on the server {} in {}.", r.host, r.root),
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools(r) }),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(r, name, &args) {
                Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
                Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            }
        }
        _ => return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("method not found: {method}") } })),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let local = args.iter().any(|a| a == "--local");
    let r = Remote {
        ctl: if local { None } else { get("--ctl") },
        host: get("--host").unwrap_or_else(|| "localhost".into()),
        root: get("--root").unwrap_or_else(|| "~".into()),
        name: get("--name").unwrap_or_else(|| "server".into()),
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(resp) = handle(&r, &msg) {
            let _ = writeln!(stdout, "{resp}");
            let _ = stdout.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(root: &std::path::Path) -> Remote {
        Remote { ctl: None, host: "localhost".into(), root: root.to_string_lossy().into(), name: "test".into() }
    }

    #[test]
    fn edit_rules() {
        assert_eq!(apply_edit("a b a", "b", "c", false).unwrap().0, "a c a");
        assert!(apply_edit("a b a", "a", "c", false).unwrap_err().contains("2 times"));
        assert_eq!(apply_edit("a b a", "a", "c", true).unwrap(), ("c b c".into(), 2));
        assert!(apply_edit("x", "y", "z", false).is_err());
    }

    #[test]
    fn paths() {
        assert_eq!(path_expr("~/app", "src/a.ts"), "~/'app/src/a.ts'");
        assert_eq!(path_expr("/srv/app", "/etc/hosts"), "'/etc/hosts'");
        assert_eq!(path_expr("/srv/my app", "."), "'/srv/my app'");
    }

    #[test]
    fn protocol_and_tools_end_to_end() {
        let dir = std::env::temp_dir().join(format!("fleet-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/fee.ts"), "export const fee = 1;\nexport const label = 'Pix';\n").unwrap();
        let r = local(&dir);

        let init = handle(&r, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}})).unwrap();
        assert_eq!(init["result"]["capabilities"], json!({"tools":{}}));
        assert!(handle(&r, &json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let list = handle(&r, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 7);

        let call = |name: &str, args: Value| {
            let v = handle(&r, &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap();
            (v["result"]["content"][0]["text"].as_str().unwrap().to_string(), v["result"]["isError"].as_bool().unwrap())
        };
        let (t, err) = call("read", json!({"path":"src/fee.ts"}));
        assert!(!err && t.contains("     1\texport const fee = 1;"), "{t}");
        let (_, err) = call("edit", json!({"path":"src/fee.ts","old_string":"fee = 1","new_string":"fee = 2"}));
        assert!(!err);
        assert!(std::fs::read_to_string(dir.join("src/fee.ts")).unwrap().contains("fee = 2"));
        let (_, err) = call("write", json!({"path":"src/new dir/x.txt","content":"it's here\n"}));
        assert!(!err);
        assert_eq!(std::fs::read_to_string(dir.join("src/new dir/x.txt")).unwrap(), "it's here\n");
        let (t, _) = call("glob", json!({"pattern":"**/*.ts"}));
        assert!(t.contains("src/fee.ts"), "{t}");
        let (t, _) = call("grep", json!({"pattern":"label","path":"src"}));
        assert!(t.contains("fee.ts:2:"), "{t}");
        let (t, _) = call("bash", json!({"command":"pwd && echo oops >&2 && exit 3"}));
        assert!(t.contains("fleet-mcp-") && t.contains("oops") && t.contains("(exit code 3)"), "{t}");
        let (t, _) = call("bash", json!({"command":"sleep 5","timeout_ms":1000}));
        assert!(t.contains("timed out"), "{t}");
        let (t, err) = call("read", json!({"file_path":"src/fee.ts"}));
        assert!(!err && t.contains("fee = 2"), "file_path alias: {t}");
        let (t, err) = call("read", json!({"path":"missing.txt"}));
        assert!(err, "{t}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn not_connected_is_explained() {
        let r = Remote { ctl: Some("/nonexistent/x.sock".into()), host: "h".into(), root: "~".into(), name: "prod".into() };
        assert!(call_tool(&r, "ls", &json!({})).unwrap_err().contains("Not connected to prod"));
    }
}

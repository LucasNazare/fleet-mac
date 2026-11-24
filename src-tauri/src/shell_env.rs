//! Environment of the user's login + interactive shell, captured once, so PATH,
//! nvm, asdf etc. match what they get in kitty.
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

static ENV: OnceLock<HashMap<String, String>> = OnceLock::new();
const MARKER: &[u8] = b"__FLEET_ENV_BEGIN__\n";

/// Variables that would confuse a nested claude or leak fleet internals.
const STRIP: &[&str] = &["FLEET_SESSION_ID", "FLEET_SOCKET", "VSCODE_INJECTION", "TERM_SESSION_ID", "CLAUDE_PID", "CLAUDE_EFFORT"];

/// Runtime markers a parent Claude Code process sets for its children (present when
/// fleet itself was launched from inside Claude Code). Legit user settings such as
/// CLAUDE_CODE_USE_BEDROCK come back from the user's rc files during capture.
pub fn is_inherited_claude_marker(k: &str) -> bool {
    k == "CLAUDECODE" || k.starts_with("CLAUDE_CODE_") || STRIP.contains(&k)
}

fn capture() -> HashMap<String, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let clean: HashMap<String, String> = std::env::vars().filter(|(k, _)| !is_inherited_claude_marker(k)).collect();
    let child = Command::new(&shell)
        .env_clear()
        .envs(&clean)
        .args(["-ilc", "printf '\\n__FLEET_ENV_BEGIN__\\n'; /usr/bin/env -0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut env = clean;
    if let Ok(child) = child {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        if let Ok(Ok(out)) = rx.recv_timeout(Duration::from_secs(8)) {
            if let Some(pos) = out.stdout.windows(MARKER.len()).rposition(|w| w == MARKER) {
                let captured: HashMap<String, String> = out.stdout[pos + MARKER.len()..]
                    .split(|b| *b == 0)
                    .filter_map(|kv| {
                        let s = String::from_utf8_lossy(kv);
                        let (k, v) = s.split_once('=')?;
                        Some((k.to_string(), v.to_string()))
                    })
                    .collect();
                if captured.contains_key("PATH") {
                    env = captured;
                }
            }
        }
    }
    for k in STRIP {
        env.remove(*k);
    }
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env.insert("TERM_PROGRAM".into(), "fleet".into());
    env.entry("LANG".into()).or_insert_with(|| "en_US.UTF-8".into());
    env
}

pub fn login_env() -> &'static HashMap<String, String> {
    ENV.get_or_init(capture)
}

pub fn which(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        let p = PathBuf::from(bin);
        return p.is_file().then_some(p);
    }
    let path = login_env().get("PATH").cloned().unwrap_or_default();
    let home = dirs::home_dir().unwrap_or_default();
    path.split(':')
        .map(PathBuf::from)
        .chain([home.join(".claude/local"), home.join(".local/bin"), PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")])
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

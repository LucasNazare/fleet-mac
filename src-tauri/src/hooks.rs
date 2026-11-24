//! Claude Code hook payloads (as forwarded by `fleet-hook`) and the generated
//! `fleet-hooks.json` settings file passed with `claude --settings`.
use serde::Deserialize;
use serde_json::{json, Value};

/// Events fleet subscribes to.
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

/// One line on the socket, written by `fleet-hook`.
#[derive(Debug, Deserialize)]
pub struct Envelope {
    pub fleet_session_id: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HookEvent {
    SessionStart { source: String },
    UserPromptSubmit { prompt: String },
    PreToolUse { tool: String, file: Option<String> },
    PostToolUse { tool: String, file: Option<String> },
    PermissionRequest { question: String },
    Notification { kind: String, message: String },
    Stop { last_message: String },
    StopFailure { message: String },
    SessionEnd { reason: String },
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub claude_session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub model: Option<String>,
    pub event: HookEvent,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit", "mcp__fleet_remote__edit", "mcp__fleet_remote__write"];

fn edited_file(tool: &str, input: &Value) -> Option<String> {
    if !EDIT_TOOLS.contains(&tool) {
        return None;
    }
    if tool.starts_with("mcp__fleet_remote__") {
        // Remote file: recorded with a marker so it's never mistaken for a local path.
        return input.get("path").and_then(|v| v.as_str()).map(|p| format!("remote:{p}"));
    }
    input
        .get("file_path")
        .or_else(|| input.get("notebook_path"))
        .and_then(|v| v.as_str())
        .map(|x| x.to_string())
}

fn truncate(t: &str, n: usize) -> String {
    let one_line = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > n {
        one_line.chars().take(n - 1).collect::<String>() + "…"
    } else {
        one_line
    }
}

/// Human question for a permission request, e.g. "run ./deploy.sh all?".
pub fn describe_permission(tool: &str, input: &Value) -> String {
    let q = match tool {
        "Bash" => format!("run {}", s(input, "command")),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            let f = edited_file(tool, input).unwrap_or_default();
            let name = std::path::Path::new(&f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(f);
            format!("{} {}", if tool == "Write" { "write" } else { "edit" }, name)
        }
        "WebFetch" => format!("fetch {}", s(input, "url")),
        "mcp__fleet_remote__bash" => format!("run {} on the server", s(input, "command")),
        "mcp__fleet_remote__edit" | "mcp__fleet_remote__write" => format!("{} {} on the server", if tool.ends_with("write") { "write" } else { "edit" }, s(input, "path")),
        t => format!("use {}", t),
    };
    truncate(&format!("{}?", q.trim()), 90)
}

pub fn parse(payload: &Value) -> Parsed {
    let name = s(payload, "hook_event_name");
    let tool = s(payload, "tool_name");
    let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
    let event = match name.as_str() {
        "SessionStart" => HookEvent::SessionStart { source: s(payload, "source") },
        "UserPromptSubmit" => HookEvent::UserPromptSubmit { prompt: s(payload, "prompt") },
        "PreToolUse" => HookEvent::PreToolUse { file: edited_file(&tool, &input), tool },
        "PostToolUse" => HookEvent::PostToolUse { file: edited_file(&tool, &input), tool },
        "PermissionRequest" => HookEvent::PermissionRequest { question: describe_permission(&tool, &input) },
        "Notification" => HookEvent::Notification {
            kind: s(payload, "notification_type"),
            message: s(payload, "message"),
        },
        "Stop" => HookEvent::Stop { last_message: s(payload, "last_assistant_message") },
        "StopFailure" => HookEvent::StopFailure {
            message: payload
                .get("error")
                .map(|e| e.as_str().map(|x| x.to_string()).unwrap_or_else(|| e.to_string()))
                .unwrap_or_default(),
        },
        "SessionEnd" => HookEvent::SessionEnd { reason: s(payload, "reason") },
        other => HookEvent::Other(other.to_string()),
    };
    let opt = |k: &str| payload.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty()).map(|v| v.to_string());
    Parsed {
        claude_session_id: opt("session_id"),
        transcript_path: opt("transcript_path"),
        model: payload
            .get("model")
            .and_then(|m| m.as_str().map(|x| x.to_string()).or_else(|| m.get("id").and_then(|i| i.as_str()).map(|x| x.to_string()))),
        event,
    }
}

/// Settings JSON registering `fleet-hook` for every event fleet listens to.
/// Passed via `--settings`, so the user's own hooks keep running alongside.
pub fn settings_json(hook_bin: &str) -> Value {
    let quoted = format!("'{}'", hook_bin.replace('\'', "'\\''"));
    let mut hooks = serde_json::Map::new();
    for ev in EVENTS {
        hooks.insert(
            ev.to_string(),
            json!([{ "hooks": [{ "type": "command", "command": quoted, "timeout": 5 }] }]),
        );
    }
    json!({ "hooks": hooks })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_permission_request() {
        let p = parse(&json!({
            "hook_event_name": "PermissionRequest", "session_id": "abc",
            "transcript_path": "/t.jsonl", "tool_name": "Bash",
            "tool_input": { "command": "./deploy.sh all" }
        }));
        assert_eq!(p.claude_session_id.as_deref(), Some("abc"));
        assert_eq!(p.event, HookEvent::PermissionRequest { question: "run ./deploy.sh all?".into() });
    }

    #[test]
    fn parses_post_tool_use_file() {
        let p = parse(&json!({
            "hook_event_name": "PostToolUse", "tool_name": "Edit",
            "tool_input": { "file_path": "/r/src/fee.ts", "old_string": "a", "new_string": "b" }
        }));
        assert_eq!(p.event, HookEvent::PostToolUse { tool: "Edit".into(), file: Some("/r/src/fee.ts".into()) });
        let p = parse(&json!({ "hook_event_name": "PostToolUse", "tool_name": "Read", "tool_input": { "file_path": "/x" } }));
        assert_eq!(p.event, HookEvent::PostToolUse { tool: "Read".into(), file: None });
    }

    #[test]
    fn parses_notification_and_stop() {
        let p = parse(&json!({ "hook_event_name": "Notification", "notification_type": "idle_prompt", "message": "waiting" }));
        assert_eq!(p.event, HookEvent::Notification { kind: "idle_prompt".into(), message: "waiting".into() });
        let p = parse(&json!({ "hook_event_name": "Stop", "last_assistant_message": "Done. Ship it?" }));
        assert_eq!(p.event, HookEvent::Stop { last_message: "Done. Ship it?".into() });
    }

    #[test]
    fn remote_tools() {
        let p = parse(&json!({ "hook_event_name": "PostToolUse", "tool_name": "mcp__fleet_remote__edit", "tool_input": { "path": "src/a.ts", "old_string": "a", "new_string": "b" } }));
        assert_eq!(p.event, HookEvent::PostToolUse { tool: "mcp__fleet_remote__edit".into(), file: Some("remote:src/a.ts".into()) });
        let p = parse(&json!({ "hook_event_name": "PermissionRequest", "tool_name": "mcp__fleet_remote__bash", "tool_input": { "command": "pm2 restart api" } }));
        assert_eq!(p.event, HookEvent::PermissionRequest { question: "run pm2 restart api on the server?".into() });
    }

    #[test]
    fn tolerates_garbage() {
        let p = parse(&json!({ "nope": 1 }));
        assert_eq!(p.event, HookEvent::Other("".into()));
        let p = parse(&json!({ "hook_event_name": "SessionStart", "model": { "id": "claude-x" } }));
        assert_eq!(p.model.as_deref(), Some("claude-x"));
    }

    #[test]
    fn settings_registers_every_event() {
        let v = settings_json("/Apps/fleet's/fleet-hook");
        for ev in EVENTS {
            let cmd = v["hooks"][ev][0]["hooks"][0]["command"].as_str().unwrap();
            assert_eq!(cmd, "'/Apps/fleet'\\''s/fleet-hook'");
        }
    }
}

//! Reads the Claude Code transcript JSONL for the context meter.
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};

const TAIL_BYTES: u64 = 1024 * 1024;

#[derive(Debug, PartialEq)]
pub struct Usage {
    pub tokens: u64,
    pub model: Option<String>,
}

/// Newest main-thread assistant usage entry in the given JSONL text.
pub fn last_usage(text: &str) -> Option<Usage> {
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        let msg = v.get("message")?;
        let Some(u) = msg.get("usage") else { continue };
        let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let tokens = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
        if tokens == 0 {
            continue;
        }
        return Some(Usage { tokens, model: msg.get("model").and_then(|m| m.as_str()).map(|s| s.to_string()) });
    }
    None
}

pub fn context_window(model: Option<&str>, tokens: u64) -> u64 {
    let m = model.unwrap_or("").to_ascii_lowercase();
    if m.contains("[1m]") || m.contains("-1m") || tokens > 200_000 { 1_000_000 } else { 200_000 }
}

pub fn context_pct(path: &str) -> Option<f32> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let u = last_usage(&text)?;
    let w = context_window(u.model.as_deref(), u.tokens);
    Some(((u.tokens as f64 / w as f64) * 100.0).min(100.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_newest_main_usage() {
        let t = [
            r#"{"type":"assistant","message":{"model":"claude-a","usage":{"input_tokens":10,"cache_read_input_tokens":1000,"cache_creation_input_tokens":0,"output_tokens":5}}}"#,
            r#"{"type":"user","message":{"content":"hi"}}"#,
            r#"{"type":"assistant","message":{"model":"claude-b","usage":{"input_tokens":2,"cache_read_input_tokens":50000,"cache_creation_input_tokens":8,"output_tokens":9}}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":999999}}}"#,
            "not json",
        ]
        .join("\n");
        assert_eq!(last_usage(&t), Some(Usage { tokens: 50010, model: Some("claude-b".into()) }));
    }

    #[test]
    fn window_size() {
        assert_eq!(context_window(Some("claude-x"), 1000), 200_000);
        assert_eq!(context_window(Some("claude-x[1m]"), 1000), 1_000_000);
        assert_eq!(context_window(None, 300_000), 1_000_000);
    }
}

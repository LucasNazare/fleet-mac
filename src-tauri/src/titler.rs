//! Live titles and recaps: a background `claude -p` with a small model.
use serde::Deserialize;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Deserialize, PartialEq)]
pub struct TitleRecap {
    pub title: String,
    #[serde(default)]
    pub recap: Vec<String>,
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect::<String>() + " …" }
}

pub fn build_prompt(last_prompt: &str, last_message: &str) -> String {
    format!(
        "You name terminal tabs for a developer running many Claude Code sessions.\n\
         Given the developer's latest request and the assistant's final reply, respond with ONLY a JSON object:\n\
         {{\"title\": \"<at most 6 words, specific, no trailing period>\", \"recap\": [\"<3 to 5 short bullets of what was done or where it stands>\"]}}\n\n\
         <request>\n{}\n</request>\n\n<reply>\n{}\n</reply>",
        clip(last_prompt, 4000),
        clip(last_message, 6000)
    )
}

/// Pulls the JSON object out of the model's answer (tolerates code fences / prose).
pub fn parse_answer(text: &str) -> Option<TitleRecap> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let mut tr: TitleRecap = serde_json::from_str(&text[start..=end]).ok()?;
    tr.title = tr.title.trim().trim_end_matches('.').split_whitespace().take(6).collect::<Vec<_>>().join(" ");
    tr.recap.truncate(5);
    (!tr.title.is_empty()).then_some(tr)
}

/// One-off `claude -p` (hooks disabled, nothing persisted) returning the result text.
pub fn run_claude_p(claude: &Path, model: &str, cwd: &Path, prompt: &str) -> anyhow::Result<String> {
    let mut cmd = Command::new(claude);
    cmd.args([
        "-p",
        "--model",
        model,
        "--output-format",
        "json",
        "--no-session-persistence",
        "--settings",
        r#"{"disableAllHooks":true}"#,
    ])
    .current_dir(cwd)
    .env_clear()
    .envs(crate::shell_env::login_env())
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    child.stdin.take().unwrap().write_all(prompt.as_bytes())?;
    let out = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v = serde_json::from_str::<serde_json::Value>(&stdout).ok();
    if let Some(v) = &v {
        if v.get("is_error").and_then(|b| b.as_bool()) == Some(true) {
            anyhow::bail!("{}", v.get("result").and_then(|r| r.as_str()).unwrap_or("claude -p failed"));
        }
    }
    let result = v.and_then(|v| v.get("result").and_then(|r| r.as_str()).map(|s| s.to_string()));
    match result {
        Some(r) => Ok(r),
        None if out.status.success() => Ok(stdout.to_string()),
        None => anyhow::bail!("claude -p failed: {}", String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("")),
    }
}

pub fn generate(claude: &Path, model: &str, cwd: &Path, last_prompt: &str, last_message: &str) -> anyhow::Result<TitleRecap> {
    let result = run_claude_p(claude, model, cwd, &build_prompt(last_prompt, last_message))?;
    parse_answer(&result).ok_or_else(|| anyhow::anyhow!("unparseable title response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json() {
        let t = "```json\n{\"title\": \"Fix Pix fee labels in checkout page today.\", \"recap\": [\"a\",\"b\",\"c\"]}\n```";
        let r = parse_answer(t).unwrap();
        assert_eq!(r.title, "Fix Pix fee labels in checkout");
        assert_eq!(r.recap.len(), 3);
        assert!(parse_answer("no json here").is_none());
    }
}

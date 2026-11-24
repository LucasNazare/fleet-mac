//! Opt-in chat sync stores transcripts on fleet-state (private repos only). Secrets are
//! removed first: known token shapes, private keys, `password=`-style values and the
//! values in the project's .env files.
use regex::Regex;
use std::sync::OnceLock;

fn patterns() -> &'static [(Regex, &'static str)] {
    static P: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    P.get_or_init(|| {
        [
            (r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----", "[redacted private key]"),
            (r"(?:sk|rk|pk)-(?:ant-|proj-|live_|test_)?[A-Za-z0-9_\-]{20,}", "[redacted]"),
            (r"gh[pousr]_[A-Za-z0-9]{30,}", "[redacted]"),
            (r"github_pat_[A-Za-z0-9_]{20,}", "[redacted]"),
            (r"AKIA[0-9A-Z]{16}", "[redacted]"),
            (r"xox[abprs]-[A-Za-z0-9\-]{10,}", "[redacted]"),
            (r"AIza[0-9A-Za-z_\-]{35}", "[redacted]"),
            (r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}", "[redacted jwt]"),
            (r"(?i)((?:password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|client[_-]?secret)\\?[\x22']?\s*[:=]\s*\\?[\x22']?)[^\s\x22'\\,}]{6,}", "${1}[redacted]"),
            (r"(?i)([a-z][a-z0-9+.\-]*://[^:/\s\x22']+:)[^@\s\x22']+@", "${1}[redacted]@"),
        ]
        .into_iter()
        .map(|(p, r)| (Regex::new(p).expect("valid redaction regex"), r))
        .collect()
    })
}

/// Remove secrets from text. `known` are literal secret values (e.g. from .env files).
pub fn redact(text: &str, known: &[String]) -> String {
    let mut out = text.to_string();
    for v in known.iter().filter(|v| v.len() >= 8) {
        out = out.replace(v.as_str(), "[redacted]");
        // The same value JSON-escaped inside a transcript line.
        let esc = serde_json::to_string(v).unwrap_or_default();
        let esc = esc.trim_matches('"');
        if esc != v {
            out = out.replace(esc, "[redacted]");
        }
    }
    for (re, rep) in patterns() {
        out = re.replace_all(&out, *rep).into_owned();
    }
    out
}

/// Values of every KEY=VALUE in the folder's .env* files.
pub fn dotenv_secrets(dirs: &[std::path::PathBuf]) -> Vec<String> {
    let mut v = vec![];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(".env") {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    v.extend(crate::proc_runtime::parse_dotenv(&t).into_iter().map(|(_, val)| val));
                }
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn removes_secrets() {
        let t = r#"{"text":"key sk-ant-api03-abcdefghijklmnopqrstuvwx and ghp_abcdefghijklmnopqrstuvwxyz0123456789 AKIAABCDEFGHIJKLMNOP; DATABASE_URL=postgres://user:hunter2pass@db/x; \"password\": \"s3cretvalue\"; mine: supersecretvalue123"}"#;
        let r = redact(t, &["supersecretvalue123".into()]);
        for leak in ["sk-ant-api03", "ghp_", "AKIA", "hunter2pass", "s3cretvalue", "supersecretvalue123"] {
            assert!(!r.contains(leak), "{leak} leaked: {r}");
        }
        assert!(r.contains("postgres://user:[redacted]@db/x"), "{r}");
        assert!(serde_json::from_str::<serde_json::Value>(&r).is_ok(), "still valid JSON: {r}");
        assert_eq!(redact("normal text, nothing here", &[]), "normal text, nothing here");
    }
}

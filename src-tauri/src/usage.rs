//! Token spend from Claude Code transcripts, priced at Anthropic API list rates, plus
//! the append-only `metrics.jsonl` log behind the per-project / per-day usage report.
//!
//! Costs are estimates of what the tokens would cost on the API. On a Claude
//! subscription they show relative spend, not a bill.
use crate::model::{Cursor, SessionMetrics, Tally};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- pricing

/// USD per million tokens; cache reads are a per-model fraction of input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub read_mult: f64,
}

const fn p(input: f64, output: f64, read_mult: f64) -> Price {
    Price { input, output, read_mult }
}

/// List price for a model id as it appears in transcripts (any provider prefix,
/// date suffix or `[1m]` tag). `prompt_tokens` selects Haiku 5.5's long-prompt tier.
pub fn price(model: &str, prompt_tokens: u64) -> Option<Price> {
    let m = model.to_ascii_lowercase();
    let has = |k: &str| m.contains(k);
    Some(if has("fable-5-2") || has("mythos-5-2") {
        p(8.0, 40.0, 0.025)
    } else if has("fable-5-1") || has("mythos-5-1") {
        p(10.0, 50.0, 0.025)
    } else if has("fable-5") || has("mythos-5") {
        p(10.0, 50.0, 0.1)
    } else if has("opus-5-5") {
        p(4.0, 20.0, 0.05)
    } else if has("opus-5") || has("opus-4-8") || has("opus-4-7") || has("opus-4-6") || has("opus-4-5") {
        p(5.0, 25.0, 0.1)
    } else if has("opus-4") || has("opus-3") || has("3-opus") {
        p(15.0, 75.0, 0.1)
    } else if has("sonnet-5-5") {
        p(2.0, 10.0, 0.05)
    } else if has("sonnet-5") {
        p(2.0, 10.0, 0.1)
    } else if has("sonnet") {
        p(3.0, 15.0, 0.1)
    } else if has("haiku-5-5") {
        if prompt_tokens > 100_000 { p(0.5, 2.5, 0.1) } else { p(0.1, 0.5, 0.1) }
    } else if has("haiku-3-5") || has("3-5-haiku") {
        p(0.8, 4.0, 0.1)
    } else if has("haiku") {
        p(1.0, 5.0, 0.1)
    } else {
        return None;
    })
}

/// One API response's usage.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub write_5m: u64,
    pub write_1h: u64,
    pub fast: bool,
}

impl Usage {
    pub fn from_json(u: &Value) -> Usage {
        let n = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let write = n(u, "cache_creation_input_tokens");
        let (write_5m, write_1h) = match u.get("cache_creation") {
            Some(cc) => {
                let h = n(cc, "ephemeral_1h_input_tokens");
                (write.saturating_sub(h), h)
            }
            None => (write, 0),
        };
        Usage {
            input: n(u, "input_tokens"),
            output: n(u, "output_tokens"),
            cache_read: n(u, "cache_read_input_tokens"),
            write_5m,
            write_1h,
            fast: u.get("speed").and_then(|s| s.as_str()) == Some("fast"),
        }
    }

    fn prompt(&self) -> u64 {
        self.input + self.cache_read + self.write_5m + self.write_1h
    }

    pub fn cost(&self, model: &str) -> Option<f64> {
        let pr = price(model, self.prompt())?;
        let input = self.input as f64 * pr.input
            + self.write_5m as f64 * pr.input * 1.25
            + self.write_1h as f64 * pr.input * 2.0
            + self.cache_read as f64 * pr.input * pr.read_mult;
        let usd = (input + self.output as f64 * pr.output) / 1_000_000.0;
        Some(if self.fast { usd * 2.0 } else { usd })
    }

    pub fn tokens(&self) -> u64 {
        self.prompt() + self.output
    }

    /// Adds to the tally; returns the cost added.
    pub fn add_to(&self, t: &mut Tally, model: &str) -> f64 {
        t.input += self.input;
        t.output += self.output;
        t.cache_read += self.cache_read;
        t.cache_write += self.write_5m + self.write_1h;
        match self.cost(model) {
            Some(c) => {
                t.cost += c;
                c
            }
            None => {
                t.unpriced += self.tokens();
                0.0
            }
        }
    }
}

// ---------------------------------------------------------------- transcript scan

const RECENT: usize = 32;

/// New usage per local day (of the transcript line's timestamp), for metrics.jsonl.
pub type Days = BTreeMap<String, Event>;

/// Count one transcript line. `sub_file`: the line comes from a sub-agent transcript.
/// Returns true if it added usage.
pub fn ingest_line(m: &mut SessionMetrics, cur: &mut Cursor, line: &str, sub_file: bool, days: &mut Days) -> bool {
    if !line.contains("\"usage\"") {
        return false;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return false };
    if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return false;
    }
    let Some(msg) = v.get("message") else { return false };
    let Some(u) = msg.get("usage") else { return false };
    // Claude Code writes a line per content block, each with the same usage.
    let key = v.get("requestId").and_then(|x| x.as_str()).or_else(|| msg.get("id").and_then(|x| x.as_str()));
    if let Some(k) = key {
        if cur.recent.iter().any(|r| r == k) {
            return false;
        }
        cur.recent.push(k.to_string());
        if cur.recent.len() > RECENT {
            cur.recent.remove(0);
        }
    }
    let usage = Usage::from_json(u);
    if usage == Usage::default() {
        return false;
    }
    let model = msg.get("model").and_then(|x| x.as_str()).unwrap_or("");
    let sub = sub_file || v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true);
    let cost = usage.add_to(if sub { &mut m.sub } else { &mut m.main }, model);
    let ms = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(crate::model::now_ms);
    let day = local_day(ms);
    let e = days.entry(day.clone()).or_insert_with(|| Event { day, ..Default::default() });
    e.t = e.t.max(ms);
    if sub {
        e.sub_cost += cost;
        e.sub_tokens += usage.tokens();
    } else {
        e.cost += cost;
        e.tokens += usage.tokens();
    }
    true
}

/// Read the new complete lines of `path` from its cursor.
fn scan_file(m: &mut SessionMetrics, path: &Path, sub_file: bool, days: &mut Days) -> bool {
    let key = path.to_string_lossy().to_string();
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let len = f.metadata().map(|x| x.len()).unwrap_or(0);
    let mut cur = m.cursors.remove(&key).unwrap_or_default();
    if len < cur.offset {
        cur.offset = 0; // rewritten; `recent` still guards the tail we counted
    }
    let mut changed = false;
    if len > cur.offset && f.seek(SeekFrom::Start(cur.offset)).is_ok() {
        let mut buf = Vec::with_capacity((len - cur.offset) as usize);
        if f.take(len - cur.offset).read_to_end(&mut buf).is_ok() {
            // Only whole lines; a half-written last line is picked up next time.
            let end = buf.iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
            for line in String::from_utf8_lossy(&buf[..end]).lines() {
                changed |= ingest_line(m, &mut cur, line, sub_file, days);
            }
            cur.offset += end as u64;
        }
    }
    m.cursors.insert(key, cur);
    changed
}

/// `<dir>/<uuid>.jsonl` → `<dir>/<uuid>/subagents/*.jsonl`.
pub fn subagent_files(transcript: &Path) -> Vec<PathBuf> {
    let dir = transcript.with_extension("").join("subagents");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Bring `m` up to date with the session's transcript and its sub-agent transcripts.
/// Returns the new usage per day (empty if nothing changed).
pub fn scan(m: &mut SessionMetrics, transcript: &Path) -> Days {
    let mut days = Days::new();
    scan_file(m, transcript, false, &mut days);
    for f in subagent_files(transcript) {
        scan_file(m, &f, true, &mut days);
    }
    days
}

/// Serializes scans so two refreshes of one session never count the same lines.
pub static SCAN_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

// ---------------------------------------------------------------- metrics log

/// One line of metrics.jsonl: a usage delta, a finished turn, or both.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Event {
    pub t: i64,
    /// Local calendar day, YYYY-MM-DD.
    pub day: String,
    pub project: String,
    pub session: String,
    pub cost: f64,
    pub sub_cost: f64,
    pub tokens: u64,
    pub sub_tokens: u64,
    pub turn_ms: Option<i64>,
    pub wait_ms: Option<i64>,
}

pub fn local_day(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

fn log_path() -> PathBuf {
    crate::persist::app_dir().join("metrics.jsonl")
}

/// Append events; `t`/`day` default to now when unset.
pub fn append(events: impl IntoIterator<Item = Event>, project: &str, session: &str) {
    let now = crate::model::now_ms();
    let mut out = String::new();
    for mut e in events {
        if e.t == 0 {
            e.t = now;
        }
        if e.day.is_empty() {
            e.day = local_day(e.t);
        }
        e.project = project.to_string();
        e.session = session.to_string();
        if let Ok(line) = serde_json::to_string(&e) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    if out.is_empty() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = f.write_all(out.as_bytes());
    }
}

/// Totals for one project on one day.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DayRow {
    pub day: String,
    pub project: String,
    pub cost: f64,
    pub sub_cost: f64,
    pub tokens: u64,
    pub sub_tokens: u64,
    pub turns: u32,
    pub turn_ms: i64,
    pub wait_ms: i64,
    /// Longest turn that day.
    pub max_turn_ms: i64,
}

/// Aggregate log lines from `since_day` (inclusive), newest day first.
pub fn aggregate(text: &str, since_day: &str) -> Vec<DayRow> {
    let mut rows: BTreeMap<(String, String), DayRow> = BTreeMap::new();
    for line in text.lines() {
        let Ok(e) = serde_json::from_str::<Event>(line) else { continue };
        if e.day.as_str() < since_day {
            continue;
        }
        let r = rows.entry((e.day.clone(), e.project.clone())).or_insert_with(|| DayRow { day: e.day.clone(), project: e.project.clone(), ..Default::default() });
        r.cost += e.cost;
        r.sub_cost += e.sub_cost;
        r.tokens += e.tokens;
        r.sub_tokens += e.sub_tokens;
        if let Some(ms) = e.turn_ms {
            r.turns += 1;
            r.turn_ms += ms;
            r.wait_ms += e.wait_ms.unwrap_or(0);
            r.max_turn_ms = r.max_turn_ms.max(ms);
        }
    }
    let mut v: Vec<DayRow> = rows.into_values().collect();
    v.sort_by(|a, b| b.day.cmp(&a.day).then(b.cost.total_cmp(&a.cost)));
    v
}

pub fn report(days: u32, now: i64) -> Vec<DayRow> {
    let since = local_day(now - (days.max(1) as i64 - 1) * 86_400_000);
    let text = std::fs::read_to_string(log_path()).unwrap_or_default();
    aggregate(&text, &since)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(req: &str, model: &str, side: bool, input: u64, read: u64, write1h: u64, out: u64) -> String {
        serde_json::json!({
            "type": "assistant", "isSidechain": side, "requestId": req, "timestamp": "2026-11-20T12:00:00.000Z",
            "message": {"id": format!("msg_{req}"), "model": model, "usage": {
                "input_tokens": input, "cache_read_input_tokens": read,
                "cache_creation_input_tokens": write1h, "output_tokens": out,
                "cache_creation": {"ephemeral_1h_input_tokens": write1h, "ephemeral_5m_input_tokens": 0}
            }}
        })
        .to_string()
    }

    #[test]
    fn prices() {
        assert_eq!(price("claude-opus-5-5", 0), Some(p(4.0, 20.0, 0.05)));
        assert_eq!(price("claude-opus-5-5[1m]", 0), Some(p(4.0, 20.0, 0.05)));
        assert_eq!(price("claude-opus-4-8", 0).unwrap().input, 5.0);
        assert_eq!(price("claude-opus-4-1-20250805", 0).unwrap().input, 15.0);
        assert_eq!(price("us.anthropic.claude-sonnet-4-6", 0).unwrap().input, 3.0);
        assert_eq!(price("claude-sonnet-5-5", 0).unwrap().read_mult, 0.05);
        assert_eq!(price("claude-haiku-5-5", 10).unwrap().input, 0.1);
        assert_eq!(price("claude-haiku-5-5", 200_000).unwrap().input, 0.5);
        assert_eq!(price("<synthetic>", 0), None);
    }

    #[test]
    fn cost_of_one_response() {
        // Opus 5.5: 1M input $4, 1M 1h-write $8, 1M read $0.20, 1M output $20.
        let u = Usage { input: 1_000_000, write_1h: 1_000_000, cache_read: 1_000_000, output: 1_000_000, ..Default::default() };
        assert!((u.cost("claude-opus-5-5").unwrap() - 32.2).abs() < 1e-9);
        let fast = Usage { fast: true, ..u };
        assert!((fast.cost("claude-opus-5-5").unwrap() - 64.4).abs() < 1e-9);
        let u5 = Usage { write_5m: 1_000_000, ..Default::default() };
        assert!((u5.cost("claude-sonnet-4-6").unwrap() - 3.75).abs() < 1e-9);
    }

    #[test]
    fn dedupes_and_splits_subagents() {
        let mut m = SessionMetrics::default();
        let mut cur = Cursor::default();
        let mut d = Days::new();
        let a = line("r1", "claude-opus-5-5", false, 10, 1000, 100, 50);
        assert!(ingest_line(&mut m, &mut cur, &a, false, &mut d));
        assert!(!ingest_line(&mut m, &mut cur, &a, false, &mut d), "same request twice counts once");
        ingest_line(&mut m, &mut cur, &line("r2", "claude-opus-5-5", true, 5, 0, 0, 5), false, &mut d);
        ingest_line(&mut m, &mut cur, &line("r3", "claude-haiku-5-5", false, 5, 0, 0, 5), true, &mut d);
        ingest_line(&mut m, &mut cur, &line("r4", "mystery-model", false, 7, 0, 0, 3), false, &mut d);
        ingest_line(&mut m, &mut cur, r#"{"type":"user","message":{"content":"hi"}}"#, false, &mut d);
        assert_eq!(m.main.tokens(), 1160 + 10);
        assert_eq!(m.main.unpriced, 10);
        assert_eq!(m.sub.tokens(), 20);
        assert_eq!(m.main.cache_write, 100);
        let day = &d[&local_day(chrono::DateTime::parse_from_rfc3339("2026-11-20T12:00:00Z").unwrap().timestamp_millis())];
        assert_eq!((day.tokens, day.sub_tokens), (1170, 20));
        assert!((day.cost - m.main.cost).abs() < 1e-12 && (day.sub_cost - m.sub.cost).abs() < 1e-12);
    }

    #[test]
    fn incremental_scan_with_subagents() {
        let dir = std::env::temp_dir().join(format!("fleet-usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("abc/subagents")).unwrap();
        let t = dir.join("abc.jsonl");
        std::fs::write(&t, format!("{}\n{}", line("r1", "claude-opus-5-5", false, 1, 0, 0, 1), &line("r2", "claude-opus-5-5", false, 1, 0, 0, 1)[..20])).unwrap();
        let mut m = SessionMetrics::default();
        assert!(!scan(&mut m, &t).is_empty());
        assert_eq!(m.main.tokens(), 2, "half-written last line waits");
        std::fs::write(&t, format!("{}\n{}\n", line("r1", "claude-opus-5-5", false, 1, 0, 0, 1), line("r2", "claude-opus-5-5", false, 1, 0, 0, 1))).unwrap();
        std::fs::write(dir.join("abc/subagents/agent-1.jsonl"), line("s1", "claude-sonnet-5-5", true, 3, 0, 0, 3) + "\n").unwrap();
        assert!(!scan(&mut m, &t).is_empty());
        assert_eq!((m.main.tokens(), m.sub.tokens()), (4, 6));
        assert!(scan(&mut m, &t).is_empty(), "nothing new");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn aggregates_by_day_and_project() {
        let ev = |day: &str, p: &str, cost: f64, turn: Option<i64>| serde_json::to_string(&Event { day: day.into(), project: p.into(), cost, tokens: 10, turn_ms: turn, wait_ms: turn.map(|_| 5), ..Default::default() }).unwrap();
        let text = [ev("2026-11-20", "a", 1.0, None), ev("2026-11-21", "a", 0.5, Some(100)), ev("2026-11-21", "a", 0.25, Some(300)), ev("2026-11-21", "b", 2.0, None), "junk".into()].join("\n");
        let rows = aggregate(&text, "2026-11-21");
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].project.as_str(), rows[0].cost), ("b", 2.0));
        assert_eq!((rows[1].turns, rows[1].turn_ms, rows[1].wait_ms, rows[1].max_turn_ms, rows[1].cost), (2, 400, 10, 300, 0.75));
    }

}

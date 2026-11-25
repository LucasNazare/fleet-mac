//! The `fleet-state` branch: an orphan branch that is fleet's database on GitHub.
//! One YAML file per session (sessions/<branch>.yml) plus deploys.yml. Written with
//! plumbing only (no checkout); concurrent writers are reconciled per file by updatedAt.
use super::gitx::{GResult, Git};
use serde::{Deserialize, Serialize};

pub const BRANCH: &str = "fleet-state";
pub const LOCAL: &str = "refs/heads/fleet-state";
pub const REMOTE: &str = "refs/remotes/origin/fleet-state";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionFile {
    pub branch: String,
    pub glyph: String,
    pub color: String,
    pub title: String,
    #[serde(default)]
    pub title_history: Vec<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub prompts: u32,
    #[serde(default)]
    pub recap: Vec<String>,
    #[serde(default)]
    pub last_machine: String,
    /// RFC 3339, UTC, seconds ("2026-11-23T17:42:10Z").
    pub updated_at: String,
    #[serde(default)]
    pub archived: bool,
    /// Main moved under this branch (a Ship) and no Mac held it: the next Mac to open it rebases.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_rebase: bool,
    /// GitHub's tip of this branch when Ship squashed it. Commits after it weren't shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipped_tip: Option<String>,
    /// Opt-in chat sync: the claude session whose transcript is in transcripts/<name>.jsonl.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
}

/// "sessions/px-a.yml" → "transcripts/px-a.jsonl"
pub fn transcript_for(session_file: &str) -> String {
    let stem = session_file.trim_start_matches("sessions/").trim_end_matches(".yml");
    format!("transcripts/{stem}.jsonl")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeployEntry {
    pub tag: String,
    pub commit: String,
    #[serde(default)]
    pub branches: Vec<String>,
    #[serde(default)]
    pub machine: String,
    pub at: String,
    /// "deployed" | "shipped" | "deploy_failed"
    #[serde(default)]
    pub result: String,
}

pub fn now_stamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn stamp_ms(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).map(|d| d.timestamp_millis()).unwrap_or(0)
}

/// "fleet/px-pix-fee-labels" → "sessions/px-pix-fee-labels.yml"
pub fn file_for(branch: &str, prefix: &str) -> String {
    let name = branch.strip_prefix(prefix).unwrap_or(branch).replace('/', "-");
    format!("sessions/{name}.yml")
}

/// Two versions of one session file: the newer `updatedAt` wins (ties keep `ours`).
pub fn merge(ours: &SessionFile, theirs: &SessionFile) -> SessionFile {
    if stamp_ms(&theirs.updated_at) > stamp_ms(&ours.updated_at) {
        theirs.clone()
    } else {
        ours.clone()
    }
}

/// Union of two deploy logs by tag, oldest first.
pub fn merge_deploys(a: &[DeployEntry], b: &[DeployEntry]) -> Vec<DeployEntry> {
    let mut out: Vec<DeployEntry> = a.to_vec();
    for e in b {
        if !out.iter().any(|x| x.tag == e.tag) {
            out.push(e.clone());
        }
    }
    out.sort_by(|x, y| x.at.cmp(&y.at).then(x.tag.cmp(&y.tag)));
    out
}

pub fn to_yaml<T: Serialize>(v: &T) -> Vec<u8> {
    serde_yaml_ng::to_string(v).unwrap_or_default().into_bytes()
}

pub fn session_from(bytes: &[u8]) -> Option<SessionFile> {
    serde_yaml_ng::from_slice(bytes).ok()
}

pub fn deploys_from(bytes: &[u8]) -> Vec<DeployEntry> {
    serde_yaml_ng::from_slice(bytes).unwrap_or_default()
}

/// Commit file changes to the local fleet-state branch (creating it, or fast-forwarding it to
/// origin's, first). One commit per write. Returns the new sha.
pub fn write(git: &Git, changes: &[(String, Option<Vec<u8>>)], msg: &str) -> GResult<String> {
    let local = git.rev(LOCAL);
    let remote = git.rev(REMOTE);
    let parent = match (&local, &remote) {
        (Some(l), Some(r)) if l != r && git.is_ancestor(l, r) => Some(r.clone()),
        (Some(l), _) => Some(l.clone()),
        (None, Some(r)) => Some(r.clone()),
        (None, None) => None,
    };
    let sha = git.commit_files(parent.as_deref(), changes, msg)?;
    git.update_ref(LOCAL, &sha, local.as_deref(), msg)?;
    Ok(sha)
}

/// All session files at a revision (default: origin's fleet-state).
pub fn read_sessions(git: &Git, rev: &str) -> Vec<(String, SessionFile)> {
    git.read_tree_files(rev, "sessions/").into_iter().filter_map(|(p, b)| session_from(&b).map(|s| (p, s))).collect()
}

pub fn read_session(git: &Git, rev: &str, path: &str) -> Option<SessionFile> {
    git.show_file(rev, path).and_then(|b| session_from(&b))
}

pub fn read_deploys(git: &Git, rev: &str) -> Vec<DeployEntry> {
    git.show_file(rev, "deploys.yml").map(|b| deploys_from(&b)).unwrap_or_default()
}

/// GitHub rejected our fleet-state push (another Mac pushed first): rebuild our changes on
/// top of origin/fleet-state, merging each file, so the next push is a fast-forward.
/// Call after fetching origin's fleet-state.
pub fn replay(git: &Git) -> GResult<()> {
    let (Some(local), Some(remote)) = (git.rev(LOCAL), git.rev(REMOTE)) else {
        if let (None, Some(r)) = (git.rev(LOCAL), git.rev(REMOTE)) {
            git.update_ref(LOCAL, &r, None, "state: take origin")?;
        }
        return Ok(());
    };
    if local == remote || git.is_ancestor(&remote, &local) {
        return Ok(());
    }
    if git.is_ancestor(&local, &remote) {
        return git.update_ref(LOCAL, &remote, Some(&local), "state: fast-forward to origin");
    }
    let base = git.read(&["merge-base", &local, &remote]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let changed: Vec<String> = match &base {
        Some(b) => git.read(&["diff-tree", "-r", "--name-only", "-z", "--no-commit-id", b, &local]).unwrap_or_default(),
        None => git.read(&["ls-tree", "-r", "--name-only", "-z", "--full-tree", &local]).unwrap_or_default(),
    }
    .split('\0')
    .filter(|p| !p.is_empty())
    .map(String::from)
    .collect();
    let mut changes = vec![];
    for path in &changed {
        let ours = git.show_file(&local, path);
        let theirs = git.show_file(&remote, path);
        let merged = match (ours, theirs) {
            (Some(o), Some(t)) if path == "deploys.yml" => Some(to_yaml(&merge_deploys(&deploys_from(&t), &deploys_from(&o)))),
            (Some(o), Some(t)) if path.starts_with("sessions/") => match (session_from(&o), session_from(&t)) {
                (Some(a), Some(b)) => Some(to_yaml(&merge(&a, &b))),
                _ => Some(o),
            },
            (Some(o), _) => Some(o),
            (None, _) => None,
        };
        changes.push((path.clone(), merged));
    }
    let n = changes.len();
    let sha = git.commit_files(Some(&remote), &changes, &format!("state: replay {n} change(s) on top of origin"))?;
    git.update_ref(LOCAL, &sha, Some(&local), "state: replay")
}

#[cfg(test)]
mod tests {
    use super::super::gitx::testutil::{sh, Fixture};
    use super::*;

    fn file(branch: &str, title: &str, at: &str) -> SessionFile {
        SessionFile { branch: branch.into(), glyph: "PX".into(), color: "#F5B95C".into(), title: title.into(), state: "done".into(), prompts: 1, updated_at: at.into(), last_machine: "MacBook Pro".into(), ..Default::default() }
    }

    #[test]
    fn newer_updated_at_wins() {
        let a = file("fleet/px-a", "old", "2026-11-23T17:42:10Z");
        let b = file("fleet/px-a", "new", "2026-11-23T17:42:11Z");
        assert_eq!(merge(&a, &b).title, "new");
        assert_eq!(merge(&b, &a).title, "new");
        let tie = file("fleet/px-a", "theirs", "2026-11-23T17:42:10Z");
        assert_eq!(merge(&a, &tie).title, "old", "ties keep ours");
    }

    #[test]
    fn file_names_and_yaml_roundtrip() {
        assert_eq!(file_for("fleet/px-pix-fee-labels", "fleet/"), "sessions/px-pix-fee-labels.yml");
        assert_eq!(file_for("feat/x/y", "fleet/"), "sessions/feat-x-y.yml");
        let f = file("fleet/px-a", "Pix fee labels → \"Tarifa do banco\"", "2026-11-23T17:42:10Z");
        let y = String::from_utf8(to_yaml(&f)).unwrap();
        assert!(y.contains("updatedAt:") && y.contains("lastMachine:") && !y.contains("needsRebase"));
        assert_eq!(session_from(y.as_bytes()).unwrap(), f);
    }

    #[test]
    fn deploy_logs_union() {
        let e = |t: &str, at: &str| DeployEntry { tag: t.into(), at: at.into(), ..Default::default() };
        let m = merge_deploys(&[e("deploy-2", "2026-11-23T14:02:00Z")], &[e("deploy-1", "2026-11-22T10:00:00Z"), e("deploy-2", "2026-11-23T14:02:00Z")]);
        assert_eq!(m.iter().map(|x| x.tag.as_str()).collect::<Vec<_>>(), vec!["deploy-1", "deploy-2"]);
    }

    #[test]
    fn write_without_checkout_and_replay_after_race() {
        let fx = Fixture::new("state");
        let a = fx.clone("a");
        let b = fx.clone("b");
        let (ga, gb) = (fx.git(&a), fx.git(&b));
        // A creates the branch.
        write(&ga, &[("sessions/px-a.yml".into(), Some(to_yaml(&file("fleet/px-a", "A1", "2026-11-23T10:00:00Z"))))], "state: PX prompt 1").unwrap();
        assert_eq!(ga.current_branch().as_deref(), Some("main"), "no checkout");
        sh(&a, "git push -q origin fleet-state");
        // B starts from origin's and writes its own session + a newer PX.
        sh(&b, "git fetch -q origin");
        write(&gb, &[("sessions/wm-b.yml".into(), Some(to_yaml(&file("fleet/wm-b", "B1", "2026-11-23T10:01:00Z"))))], "state: WM prompt 1").unwrap();
        write(&gb, &[("sessions/px-a.yml".into(), Some(to_yaml(&file("fleet/px-a", "A-from-B", "2026-11-23T10:05:00Z"))))], "state: PX from B").unwrap();
        sh(&b, "git push -q origin fleet-state");
        // A writes again with an older stamp for PX; its push is rejected.
        write(&ga, &[("sessions/px-a.yml".into(), Some(to_yaml(&file("fleet/px-a", "A2", "2026-11-23T10:02:00Z"))))], "state: PX prompt 2").unwrap();
        let rejected = std::process::Command::new("git").args(["-C", a.to_str().unwrap(), "push", "-q", "origin", "fleet-state"]).output().unwrap();
        assert!(!rejected.status.success());
        sh(&a, "git fetch -q origin");
        replay(&ga).unwrap();
        sh(&a, "git push -q origin fleet-state");
        let all = read_sessions(&ga, LOCAL);
        assert_eq!(all.len(), 2);
        let px = all.iter().find(|(p, _)| p == "sessions/px-a.yml").unwrap();
        assert_eq!(px.1.title, "A-from-B", "newer updatedAt (B's) wins");
        assert!(ga.is_ancestor(REMOTE, LOCAL));
    }
}

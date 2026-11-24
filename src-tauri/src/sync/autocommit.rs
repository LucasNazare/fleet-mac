//! After every finished prompt (Stop): commit the session's worktree to its own branch,
//! update its file on fleet-state and push both together. Tauri-free so it's testable
//! against a local bare repo.
use super::gitx::Git;
use super::presence::{self, Holder};
use super::pusher::{self, Lease, PushOutcome, Refspec};
use super::state_branch::{self, SessionFile};

pub const MAX_FILE: u64 = 5 * 1024 * 1024;

/// What the session looks like right now (read after the titler finished).
#[derive(Debug, Clone, Default)]
pub struct Snap {
    pub glyph: String,
    pub color: String,
    pub title: String,
    pub title_history: Vec<String>,
    pub state: String,
    pub recap: Vec<String>,
    pub prompts: u32,
    /// Chat sync: claude session id + redacted transcript to store next to the state file.
    pub chat: Option<(String, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Pushed,
    /// Offline: committed locally, queued.
    Waiting(String),
    /// Rebase onto GitHub's branch conflicted in these files (left in progress).
    Conflict(Vec<String>),
    /// A rebase/merge is half-finished in the worktree; nothing was done.
    Paused(String),
    /// Another Mac holds this branch.
    TakenOver(String),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub branch: String,
    pub commit: Option<String>,
    pub excluded: Vec<String>,
    pub status: Status,
    /// Our presence tag on GitHub after a successful push.
    pub presence: Option<String>,
}

/// Hold the branch's presence while pushing (see presence.rs).
#[derive(Debug, Clone)]
pub struct PresenceCtx {
    pub machine: String,
    pub install: String,
    /// Our last presence tag sha for this branch, if known.
    pub expect: Option<String>,
    /// Take it even if another Mac holds it (Take over / Continue here).
    pub take: bool,
    /// The branch was rebased on purpose ("Fix with Claude" when Ship rebases): GitHub's copy
    /// is replaced, but only while it's still this commit (a lease, never a blind force).
    pub rewritten_from: Option<String>,
}

pub struct Input<'a> {
    pub git: &'a Git,
    pub prefix: &'a str,
    pub machine: &'a str,
    /// Quit/sleep while mid-prompt: "wip:" commit, no message generation.
    pub wip: bool,
    /// Fresh session data; may block until the title is regenerated.
    pub snapshot: &'a dyn Fn() -> Snap,
    /// Commit message (subject ≤ 72 + short body) from the staged diff.
    pub message: &'a dyn Fn(&Git) -> Option<String>,
    /// Presence (only push while this Mac holds the branch).
    pub presence: Option<PresenceCtx>,
}

/// Never auto-committed: secrets and big files.
pub fn is_excluded(path: &str, size: u64) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with(".env") || name.ends_with(".pem") || name.ends_with(".key") || name.starts_with("id_rsa") || size > MAX_FILE
}

/// fleet/<glyph>-<slug of title>, unique among local and GitHub branches.
pub fn branch_name(prefix: &str, glyph: &str, title: &str, exists: &dyn Fn(&str) -> bool) -> String {
    let g: String = glyph.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let g = if g.is_empty() { "s".to_string() } else { g };
    let slug = crate::worktree::slugify(title, &|c| exists(&format!("{prefix}{g}-{c}")));
    format!("{prefix}{g}-{slug}")
}

/// Drop attribution lines a model may add; fleet writes its own trailers.
fn strip_attribution(msg: &str) -> String {
    msg.lines()
        .filter(|l| {
            let t = l.trim().to_lowercase();
            !(t.starts_with("co-authored-by:") || t.contains("generated with [claude code]") || t.starts_with("🤖 generated with"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn subject_limit(msg: &str) -> String {
    let msg = msg.trim();
    let mut lines = msg.lines();
    let subject = lines.next().unwrap_or("").trim();
    let subject: String = if subject.chars().count() > 72 { subject.chars().take(71).collect::<String>() + "…" } else { subject.to_string() };
    let body: Vec<&str> = lines.collect();
    let body = body.join("\n");
    let body = body.trim();
    if body.is_empty() {
        subject
    } else {
        format!("{subject}\n\n{body}")
    }
}

pub fn trailers(glyph: &str, title: &str, machine: &str, prompt: u32) -> String {
    let one = |s: &str| s.replace(['\n', '\r'], " ");
    format!("Fleet-Session: {}\nFleet-Title: {}\nFleet-Machine: {}\nFleet-Prompt: {prompt}", one(glyph), one(title), one(machine))
}

fn fallback_message(title: &str, files: &[String]) -> String {
    let mut m = subject_limit(title);
    if !files.is_empty() {
        m.push_str("\n\n");
        for f in files.iter().take(12) {
            m.push_str(&format!("- {f}\n"));
        }
        if files.len() > 12 {
            m.push_str(&format!("- … and {} more\n", files.len() - 12));
        }
    }
    m
}

fn branch_exists(git: &Git, name: &str) -> bool {
    git.rev(&format!("refs/heads/{name}")).is_some() || git.rev(&format!("refs/remotes/origin/{name}")).is_some()
}

pub fn conflicted(git: &Git) -> Vec<String> {
    git.read(&["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default().lines().map(String::from).collect()
}

pub fn state_file(branch: &str, snap: &Snap, machine: &str) -> SessionFile {
    SessionFile {
        branch: branch.into(),
        glyph: snap.glyph.clone(),
        color: snap.color.clone(),
        title: snap.title.clone(),
        title_history: snap.title_history.clone(),
        state: snap.state.clone(),
        prompts: snap.prompts,
        recap: snap.recap.clone(),
        last_machine: machine.into(),
        updated_at: state_branch::now_stamp(),
        archived: false,
        needs_rebase: false,
        claude_session_id: snap.chat.as_ref().map(|c| c.0.clone()),
    }
}

pub fn run(inp: &Input) -> Outcome {
    let git = inp.git;
    let mut out = Outcome { branch: git.current_branch().unwrap_or_default(), commit: None, excluded: vec![], status: Status::Pushed, presence: None };
    if let Some(op) = git.operation_in_progress() {
        out.status = Status::Paused(format!("A {op} is in progress in this worktree. Finish or abort it; sync resumes after."));
        return out;
    }
    // 1. Stage everything but the excluded files (.gitignore is respected by git add).
    if let Err(e) = git.run(&["add", "-A"]) {
        out.status = Status::Error(e.to_string());
        return out;
    }
    let staged: Vec<String> = git.read(&["diff", "--cached", "--name-only", "-z", "--diff-filter=d"]).unwrap_or_default().split('\0').filter(|s| !s.is_empty()).map(String::from).collect();
    out.excluded = staged.iter().filter(|p| is_excluded(p, std::fs::metadata(git.root.join(p)).map(|m| m.len()).unwrap_or(0))).cloned().collect();
    if !out.excluded.is_empty() {
        let mut a = vec!["reset", "-q", "--"];
        a.extend(out.excluded.iter().map(|s| s.as_str()));
        let _ = git.run(&a);
    }
    let has_changes = git.read(&["diff", "--cached", "--quiet"]).is_err();
    let files: Vec<String> = if has_changes { git.read(&["diff", "--cached", "--name-only"]).unwrap_or_default().lines().map(String::from).collect() } else { vec![] };
    // 2. Message (slow claude -p call) while the titler finishes, then fresh session data.
    let drafted = if has_changes && !inp.wip { (inp.message)(git) } else { None };
    let snap = (inp.snapshot)();
    // 3. The session's branch: named from its title on its first finished prompt.
    let branch = match git.current_branch() {
        Some(b) => b,
        None => {
            let name = branch_name(inp.prefix, &snap.glyph, &snap.title, &|n| branch_exists(git, n));
            if let Err(e) = git.run(&["switch", "-c", &name]) {
                out.status = Status::Error(e.to_string());
                return out;
            }
            name
        }
    };
    out.branch = branch.clone();
    if pusher::protected(git).contains(&branch) {
        out.status = Status::Error(format!("This worktree is on {branch}; sessions only commit to their own branch."));
        return out;
    }
    // 4. Commit with trailers.
    if has_changes {
        let body = if inp.wip {
            format!("wip: {}", snap.title)
        } else {
            drafted.map(|m| subject_limit(&strip_attribution(&m))).filter(|m| !m.is_empty()).unwrap_or_else(|| fallback_message(&snap.title, &files))
        };
        let msg = format!("{}\n\n{}\n", subject_limit(&body), trailers(&snap.glyph, &snap.title, inp.machine, snap.prompts));
        let ident = git.ident_env();
        let ident: Vec<(&str, &str)> = ident.iter().map(|(k, v)| (*k, v.as_str())).collect();
        if let Err(e) = git.run_with(&["commit", "-q", "--no-verify", "-F", "-"], &ident, Some(msg.as_bytes()), true) {
            out.status = Status::Error(e.to_string());
            return out;
        }
        out.commit = git.rev("HEAD");
    }
    // 5. The session's file on fleet-state.
    let file = state_file(&branch, &snap, inp.machine);
    let path = state_branch::file_for(&branch, inp.prefix);
    let what = if inp.wip { "wip".to_string() } else { format!("prompt {}", snap.prompts) };
    let mut changes = vec![(path.clone(), Some(state_branch::to_yaml(&file)))];
    if let Some((_, t)) = &snap.chat {
        changes.push((state_branch::transcript_for(&path), Some(t.clone())));
    }
    if let Err(e) = state_branch::write(git, &changes, &format!("state: {} {what}", snap.glyph)) {
        out.status = Status::Error(e.to_string());
        return out;
    }
    // 6. Push branch + state together.
    let (status, tag) = push_session(git, &branch, inp.presence.as_ref());
    out.status = status;
    out.presence = tag;
    out
}

/// Push the session's branch, fleet-state and (optionally) its presence in one atomic push;
/// on rejection fetch, replay state / rebase the branch onto GitHub's, and retry.
/// Returns the status and our new presence tag.
pub fn push_session(git: &Git, branch: &str, presence: Option<&PresenceCtx>) -> (Status, Option<String>) {
    let mut expect = presence.and_then(|p| p.expect.clone());
    // Another Mac created fleet-state: start ours from GitHub's.
    if git.rev(state_branch::LOCAL).is_none() {
        if let Some(r) = git.rev(state_branch::REMOTE) {
            let _ = git.update_ref(state_branch::LOCAL, &r, None, "state: take origin");
        }
    }
    let has_state = git.rev(state_branch::LOCAL).is_some();
    let branch_ref = format!("refs/heads/{branch}");
    // Only a real rewrite needs the lease; a plain fast-forward pushes as usual.
    let rewrite = presence.and_then(|p| p.rewritten_from.clone()).filter(|old| !git.is_ancestor(old, &branch_ref));
    for _ in 0..4 {
        let mut specs = vec![Refspec::branch(branch)];
        if has_state {
            specs.push(Refspec::branch(state_branch::BRANCH));
        }
        let mut leases = vec![];
        if let Some(old) = &rewrite {
            leases.push(Lease { dst: branch_ref.clone(), expect: old.clone() });
        }
        let mut tag = None;
        if let Some(p) = presence {
            let Some(head) = git.rev(&format!("refs/heads/{branch}")) else { return (Status::Error(format!("{branch} doesn't exist")), None) };
            match presence::make_tag(git, &head, &p.machine, &p.install, chrono::Utc::now().timestamp()) {
                Ok(t) => {
                    specs.push(Refspec::new(&t, &presence::remote_ref(branch)));
                    leases.push(Lease { dst: presence::remote_ref(branch), expect: expect.clone().unwrap_or_default() });
                    tag = Some(t);
                }
                Err(e) => return (Status::Error(e.to_string()), None),
            }
        }
        match pusher::push(git, &specs, &leases, None) {
            PushOutcome::Pushed => {
                let _ = git.read(&["branch", "--set-upstream-to", &format!("origin/{branch}"), branch]);
                return (Status::Pushed, tag);
            }
            PushOutcome::Offline(m) => return (Status::Waiting(m), None),
            PushOutcome::LeaseLost(refs, m) => {
                if rewrite.is_some() && refs.contains(&branch_ref) {
                    return (Status::Error(format!("GitHub's {branch} changed while it was being rebased. Fetch, then fix the conflict again.")), None);
                }
                let Some(p) = presence else { return (Status::Error(m), None) };
                let cur = presence::fetch_one(git, branch);
                match presence::holder(cur.as_ref(), &p.install, chrono::Utc::now().timestamp_millis()) {
                    Holder::Other { machine, age_ms } if !p.take => return (Status::TakenOver(format!("Active on {machine} · {}", presence::ago(age_ms))), None),
                    _ => expect = cur.map(|c| c.sha),
                }
            }
            PushOutcome::Refused(m) | PushOutcome::Failed(m) => return (Status::Error(m), None),
            PushOutcome::Rejected(refs, _) => {
                if let Err(e) = git.run(&["fetch", "--prune", "origin"]) {
                    return (if pusher::is_offline(&e.stderr) { Status::Waiting(e.to_string()) } else { Status::Error(e.to_string()) }, None);
                }
                if refs.iter().any(|r| r.ends_with(state_branch::BRANCH)) {
                    if let Err(e) = state_branch::replay(git) {
                        return (Status::Error(e.to_string()), None);
                    }
                }
                if refs.iter().any(|r| r == &format!("refs/heads/{branch}")) {
                    // GitHub has commits this Mac doesn't: rebase ours on top (pull --rebase).
                    if git.run(&["rebase", "--autostash", &format!("origin/{branch}")]).is_err() {
                        let files = conflicted(git);
                        return (Status::Conflict(if files.is_empty() { vec!["(see the Output drawer)".into()] } else { files }), None);
                    }
                }
            }
        }
    }
    (Status::Error("GitHub kept rejecting the push; try again from the git panel.".into()), None)
}

/// Refresh (or take) the branch's presence ref alone.
pub fn push_presence(git: &Git, branch: &str, p: &PresenceCtx) -> (Status, Option<String>) {
    let mut expect = p.expect.clone();
    let Some(head) = git.rev(&format!("refs/heads/{branch}")) else { return (Status::Error(format!("{branch} doesn't exist")), None) };
    for _ in 0..3 {
        let tag = match presence::make_tag(git, &head, &p.machine, &p.install, chrono::Utc::now().timestamp()) {
            Ok(t) => t,
            Err(e) => return (Status::Error(e.to_string()), None),
        };
        let lease = Lease { dst: presence::remote_ref(branch), expect: expect.clone().unwrap_or_default() };
        match pusher::push(git, &[Refspec::new(&tag, &presence::remote_ref(branch))], &[lease], None) {
            PushOutcome::Pushed => return (Status::Pushed, Some(tag)),
            PushOutcome::Offline(m) => return (Status::Waiting(m), None),
            PushOutcome::LeaseLost(..) | PushOutcome::Rejected(..) => {
                let cur = presence::fetch_one(git, branch);
                match presence::holder(cur.as_ref(), &p.install, chrono::Utc::now().timestamp_millis()) {
                    Holder::Other { machine, age_ms } if !p.take => return (Status::TakenOver(format!("Active on {machine} · {}", presence::ago(age_ms))), None),
                    _ => expect = cur.map(|c| c.sha),
                }
            }
            PushOutcome::Refused(m) | PushOutcome::Failed(m) => return (Status::Error(m), None),
        }
    }
    (Status::Error("Couldn't update presence".into()), None)
}

/// Drop our presence (session closed), only if it's still ours.
pub fn release_presence(git: &Git, branch: &str, mine: &str) -> bool {
    let lease = Lease { dst: presence::remote_ref(branch), expect: mine.to_string() };
    matches!(pusher::push(git, &[Refspec::delete(&presence::remote_ref(branch))], &[lease], None), PushOutcome::Pushed)
}

#[cfg(test)]
mod tests {
    use super::super::gitx::testutil::{sh, Fixture};
    use super::*;
    use std::path::Path;

    fn snap(title: &str, n: u32) -> Snap {
        Snap { glyph: "PX".into(), color: "#F5B95C".into(), title: title.into(), state: "done".into(), prompts: n, recap: vec!["Bank part of fees now reads \"Tarifa do banco\"".into()], ..Default::default() }
    }

    fn worktree(clone: &Path) -> std::path::PathBuf {
        sh(clone, "git fetch -q origin && git worktree add -q --detach .fleet/worktrees/s origin/main");
        clone.join(".fleet/worktrees/s")
    }

    fn input<'a>(git: &'a Git, s: &'a dyn Fn() -> Snap, m: &'a dyn Fn(&Git) -> Option<String>) -> Input<'a> {
        Input { git, prefix: "fleet/", machine: "MacBook Pro", wip: false, snapshot: s, message: m, presence: None }
    }

    #[test]
    fn exclusions() {
        for p in [".env", "app/.env.local", "certs/server.pem", "a/b.key", "id_rsa", "id_rsa.pub"] {
            assert!(is_excluded(p, 10), "{p}");
        }
        assert!(!is_excluded("src/env.ts", 10));
        assert!(!is_excluded("keys.ts", 10));
        assert!(is_excluded("video.mp4", MAX_FILE + 1));
    }

    #[test]
    fn model_attribution_is_dropped() {
        let m = subject_limit(&strip_attribution("Add fees.ts\n\nCreate the label.\n\nCo-Authored-By: Claude Haiku <noreply@anthropic.com>\n"));
        assert_eq!(m, "Add fees.ts\n\nCreate the label.");
    }

    #[test]
    fn branch_names() {
        assert_eq!(branch_name("fleet/", "PX", "Pix fee labels → \"Tarifa do banco\"", &|_| false), "fleet/px-pix-fee-labels-tarifa-do-banco");
        assert_eq!(branch_name("fleet/", "PX", "Pix", &|n| n == "fleet/px-pix"), "fleet/px-pix-2");
    }

    #[test]
    fn finished_prompt_is_committed_and_pushed_with_state() {
        let fx = Fixture::new("stop");
        let a = fx.clone("a");
        let wt = worktree(&a);
        let g = fx.git(&wt);
        std::fs::write(wt.join("fees.ts"), "export const label = 'Tarifa do banco'\n").unwrap();
        std::fs::write(wt.join(".env"), "SECRET=1\n").unwrap();
        let s = || snap("Pix fee labels", 1);
        let m = |_: &Git| Some("Rename bank fee label\n\nLabels now come from feeDescription().".to_string());
        let out = run(&input(&g, &s, &m));
        assert_eq!(out.status, Status::Pushed, "{out:?}");
        assert_eq!(out.branch, "fleet/px-pix-fee-labels");
        assert_eq!(out.excluded, vec![".env"]);
        // On "GitHub": the commit with trailers, and the session file.
        let bare = fx.git(&fx.bare);
        let msg = bare.read(&["log", "-1", "--format=%B", "fleet/px-pix-fee-labels"]).unwrap();
        assert!(msg.starts_with("Rename bank fee label\n"), "{msg}");
        for t in ["Fleet-Session: PX", "Fleet-Title: Pix fee labels", "Fleet-Machine: MacBook Pro", "Fleet-Prompt: 1"] {
            assert!(msg.contains(t), "{t} in {msg}");
        }
        let trailers = bare.read(&["log", "-1", "--format=%(trailers:key=Fleet-Prompt,valueonly)", "fleet/px-pix-fee-labels"]).unwrap();
        assert_eq!(trailers.trim(), "1", "parsed as real git trailers");
        let files = bare.read(&["ls-tree", "-r", "--name-only", "fleet/px-pix-fee-labels"]).unwrap();
        assert!(files.contains("fees.ts") && !files.contains(".env"));
        let f = state_branch::read_session(&bare, "fleet-state", "sessions/px-pix-fee-labels.yml").unwrap();
        assert_eq!((f.title.as_str(), f.prompts, f.last_machine.as_str()), ("Pix fee labels", 1, "MacBook Pro"));
        // Second prompt without changes: only the state moves.
        let head = bare.rev("fleet/px-pix-fee-labels");
        let s2 = || snap("Pix fee labels", 2);
        let out = run(&input(&g, &s2, &m));
        assert_eq!(out.status, Status::Pushed);
        assert!(out.commit.is_none());
        assert_eq!(bare.rev("fleet/px-pix-fee-labels"), head);
        assert_eq!(state_branch::read_session(&bare, "fleet-state", "sessions/px-pix-fee-labels.yml").unwrap().prompts, 2);
    }

    #[test]
    fn offline_queues_then_pushes() {
        let fx = Fixture::new("offline");
        let a = fx.clone("a");
        let wt = worktree(&a);
        let g = fx.git(&wt);
        std::fs::rename(&fx.bare, fx.dir.join("away.git")).unwrap();
        std::fs::write(wt.join("x.txt"), "x").unwrap();
        let s = || snap("Offline work", 1);
        let m = |_: &Git| None;
        let out = run(&input(&g, &s, &m));
        assert!(matches!(out.status, Status::Waiting(_)), "{out:?}");
        assert!(out.commit.is_some(), "committed locally");
        std::fs::rename(fx.dir.join("away.git"), &fx.bare).unwrap();
        assert_eq!(push_session(&g, &out.branch, None).0, Status::Pushed);
        assert!(fx.git(&fx.bare).rev(&out.branch).is_some());
        assert!(fx.git(&fx.bare).rev("fleet-state").is_some());
    }

    #[test]
    fn remote_ahead_rebases_and_conflict_needs_you() {
        let fx = Fixture::new("ahead");
        let a = fx.clone("a");
        let b = fx.clone("b");
        // A pushes the session branch; B (another Mac) adds a commit to it.
        let wa = worktree(&a);
        let ga = fx.git(&wa);
        std::fs::write(wa.join("a.txt"), "hello from A\n").unwrap();
        let s = || snap("Shared work", 1);
        let m = |_: &Git| Some("A edit".into());
        let out = run(&input(&ga, &s, &m));
        assert_eq!(out.status, Status::Pushed);
        sh(&b, &format!("git fetch -q && git switch -q -c {0} origin/{0} && echo more >> b.txt && git commit -qam 'B edit' && git push -q origin {0}", out.branch));
        // A's next prompt touches another file: rebased cleanly onto B's commit.
        std::fs::write(wa.join("c.txt"), "new\n").unwrap();
        let s2 = || snap("Shared work", 2);
        let out2 = run(&input(&ga, &s2, &m));
        assert_eq!(out2.status, Status::Pushed, "{out2:?}");
        let log = fx.git(&fx.bare).read(&["log", "--format=%s", &out.branch]).unwrap();
        assert!(log.starts_with("A edit\nB edit\nA edit"), "{log}");
        // Now both change the same line: conflict, left for the session.
        sh(&b, &format!("git pull -q --rebase origin {0} && echo B > a.txt && git commit -qam 'B a.txt' && git push -q origin {0}", out.branch));
        std::fs::write(wa.join("a.txt"), "A again\n").unwrap();
        let s3 = || snap("Shared work", 3);
        let out3 = run(&input(&ga, &s3, &m));
        assert_eq!(out3.status, Status::Conflict(vec!["a.txt".into()]));
        // Half-finished rebase: the next Stop does nothing.
        let out4 = run(&input(&ga, &s3, &m));
        assert!(matches!(out4.status, Status::Paused(_)));
    }

    #[test]
    fn two_macs_never_push_the_same_branch() {
        let fx = Fixture::new("presence-push");
        let a = fx.clone("a");
        let b = fx.clone("b");
        let wa = worktree(&a);
        let ga = fx.git(&wa);
        let ctx = |install: &str, expect: Option<String>, take: bool| PresenceCtx { machine: if install == "A" { "MacBook Pro".into() } else { "iMac".into() }, install: install.into(), expect, take, rewritten_from: None };
        std::fs::write(wa.join("x.txt"), "1").unwrap();
        let s = || snap("Shared", 1);
        let m = |_: &Git| Some("one".into());
        let mut i = input(&ga, &s, &m);
        i.presence = Some(ctx("A", None, false));
        let out = run(&i);
        assert_eq!(out.status, Status::Pushed, "{out:?}");
        let tag_a = out.presence.clone().unwrap();
        // B continues the session: takes presence.
        sh(&b, &format!("git fetch -q && git worktree add -q wb -b {0} origin/{0}", out.branch));
        let gb = fx.git(&b.join("wb"));
        let (st, tag_b) = push_session(&gb, &out.branch, Some(&ctx("B", None, true)));
        assert_eq!(st, Status::Pushed);
        assert!(tag_b.is_some());
        // A's next push is refused: taken over by B.
        std::fs::write(wa.join("x.txt"), "2").unwrap();
        let s2 = || snap("Shared", 2);
        let mut i2 = input(&ga, &s2, &m);
        i2.presence = Some(ctx("A", Some(tag_a), false));
        let out2 = run(&i2);
        assert!(matches!(&out2.status, Status::TakenOver(w) if w.starts_with("Active on iMac")), "{out2:?}");
        let bare = fx.git(&fx.bare);
        assert_eq!(bare.read(&["log", "-1", "--format=%s", &out.branch]).unwrap().trim(), "one", "A's second commit never reached GitHub");
        // A restarted (no known tag) but B's presence is stale-free: still refused; with take it wins.
        let (st, _) = push_session(&ga, &out.branch, Some(&ctx("A", None, false)));
        assert!(matches!(st, Status::TakenOver(_)));
        let (st, _) = push_session(&ga, &out.branch, Some(&ctx("A", None, true)));
        assert!(matches!(st, Status::Pushed | Status::Conflict(_)), "{st:?}");
    }

    #[test]
    fn wip_commit() {
        let fx = Fixture::new("wip");
        let a = fx.clone("a");
        let wt = worktree(&a);
        let g = fx.git(&wt);
        std::fs::write(wt.join("half.txt"), "half").unwrap();
        let s = || snap("Halfway", 4);
        let m = |_: &Git| panic!("no message generation for wip");
        let mut i = input(&g, &s, &m);
        i.wip = true;
        let out = run(&i);
        assert_eq!(out.status, Status::Pushed);
        let msg = fx.git(&fx.bare).read(&["log", "-1", "--format=%s", &out.branch]).unwrap();
        assert_eq!(msg.trim(), "wip: Halfway");
    }
}

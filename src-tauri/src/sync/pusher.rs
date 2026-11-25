//! The only place fleet pushes from (plus the git panel's manual push, which is guarded here).
//! Pushing main needs a `ship::MainPush`, which only the Ship sequence can create.
use super::gitx::Git;
use super::ship::MainPush;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use parking_lot::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub struct Refspec {
    pub src: String,
    pub dst: String,
}

impl Refspec {
    pub fn branch(name: &str) -> Refspec {
        Refspec { src: format!("refs/heads/{name}"), dst: format!("refs/heads/{name}") }
    }
    pub fn new(src: &str, dst: &str) -> Refspec {
        Refspec { src: src.into(), dst: dst.into() }
    }
    /// Delete `dst` on GitHub.
    pub fn delete(dst: &str) -> Refspec {
        Refspec { src: String::new(), dst: dst.into() }
    }
}

/// `--force-with-lease=<ref>:<expected>` (expected "" = must not exist).
#[derive(Debug, Clone)]
pub struct Lease {
    pub dst: String,
    pub expect: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PushOutcome {
    Pushed,
    /// Network/remote unreachable: queue and retry.
    Offline(String),
    /// Non-fast-forward on these destination refs.
    Rejected(Vec<String>, String),
    /// A lease failed (someone else moved that ref).
    LeaseLost(Vec<String>, String),
    /// Refused locally (e.g. main without a MainPush).
    Refused(String),
    Failed(String),
}

/// Branch names that are production: main/master and origin's default branch.
pub fn protected(git: &Git) -> Vec<String> {
    let mut v = vec!["main".to_string(), "master".to_string()];
    let d = git.default_branch();
    if !v.contains(&d) {
        v.push(d);
    }
    v
}

fn targets_protected(dst: &str, protected: &[String]) -> bool {
    let name = dst.trim_start_matches('+').strip_prefix("refs/heads/").unwrap_or(dst.trim_start_matches('+'));
    protected.iter().any(|p| p == name)
}

pub fn check(specs: &[Refspec], protected: &[String], main: Option<&MainPush>) -> Result<(), String> {
    for s in specs {
        if s.dst.is_empty() || s.src.starts_with('+') {
            return Err("fleet never force-pushes without a lease.".into());
        }
        if targets_protected(&s.dst, protected) && main.is_none() {
            return Err(format!("{} changes only through Ship (⌘⇧D).", s.dst.trim_start_matches("refs/heads/")));
        }
    }
    Ok(())
}

/// Signed out, wrong account, no access or the repo is gone: retrying won't help, the user
/// has to act. Checked before is_offline (git words several of these like network errors).
pub fn is_auth(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    [
        "authentication failed",
        "permission denied",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "invalid username or password",
        "repository not found",
        "the requested url returned error: 403",
        "the requested url returned error: 401",
        "error: 403",
        "host key verification failed",
    ]
    .iter()
    .any(|m| s.contains(m))
}

pub fn is_offline(stderr: &str) -> bool {
    if is_auth(stderr) {
        return false;
    }
    let s = stderr.to_lowercase();
    [
        "could not resolve host",
        "unable to access",
        "could not read from remote repository",
        "connection refused",
        "connection timed out",
        "operation timed out",
        "network is unreachable",
        "does not appear to be a git repository",
        "no route to host",
        "connection reset",
        "the remote end hung up unexpectedly",
        "failed to connect",
        "temporary failure in name resolution",
    ]
    .iter()
    .any(|m| s.contains(m))
}

/// Parse `git push --porcelain` output into rejected refs (by reason).
fn parse_porcelain(out: &str) -> (Vec<String>, Vec<String>) {
    let (mut rejected, mut stale) = (vec![], vec![]);
    for line in out.lines() {
        // "!\trefs/heads/a:refs/heads/a\t[rejected] (fetch first)"
        let mut parts = line.split('\t');
        let (Some(flag), Some(spec), reason) = (parts.next(), parts.next(), parts.next().unwrap_or("")) else { continue };
        if flag != "!" {
            continue;
        }
        let dst = spec.split(':').nth(1).unwrap_or(spec).to_string();
        if reason.contains("stale info") {
            stale.push(dst);
        } else if reason.contains("atomic push failed") {
            // collateral of another ref's failure
        } else {
            rejected.push(dst);
        }
    }
    (rejected, stale)
}

/// `git push --porcelain --atomic origin <specs>` with optional leases.
pub fn push(git: &Git, specs: &[Refspec], leases: &[Lease], main: Option<&MainPush>) -> PushOutcome {
    let prot = protected(git);
    if let Err(e) = check(specs, &prot, main) {
        return PushOutcome::Refused(e);
    }
    if leases.iter().any(|l| targets_protected(&l.dst, &prot)) && main.is_none() {
        return PushOutcome::Refused("fleet never force-pushes main.".into());
    }
    let mut args: Vec<String> = vec!["push".into(), "--porcelain".into()];
    if specs.len() + leases.len() > 1 {
        args.push("--atomic".into());
    }
    for l in leases {
        args.push(format!("--force-with-lease={}:{}", l.dst, l.expect));
    }
    args.push("origin".into());
    for s in specs {
        args.push(format!("{}:{}", s.src, s.dst));
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match git.run(&refs) {
        Ok(_) => PushOutcome::Pushed,
        Err(e) => {
            let (rejected, stale) = parse_porcelain(&e.stdout);
            if !stale.is_empty() {
                PushOutcome::LeaseLost(stale, e.stderr)
            } else if !rejected.is_empty() {
                PushOutcome::Rejected(rejected, e.stderr)
            } else if is_auth(&e.stderr) {
                PushOutcome::Failed(format!("GitHub refused fleet's sign-in or access to this repo — sign in again (GitHub sync dialog → Sign in, or `gh auth login`). {}", e.stderr.trim().lines().last().unwrap_or("")))
            } else if is_offline(&e.stderr) {
                PushOutcome::Offline(e.stderr.trim().lines().last().unwrap_or("offline").to_string())
            } else {
                PushOutcome::Failed(e.to_string())
            }
        }
    }
}

// ---------------------------------------------------------------- manual pushes (git panel)

fn synced_roots() -> &'static Mutex<Vec<PathBuf>> {
    static R: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(vec![]))
}

pub fn set_synced_roots(roots: Vec<PathBuf>) {
    *synced_roots().lock() = roots;
}

pub fn is_synced(root: &Path) -> bool {
    synced_roots().lock().iter().any(|r| root.starts_with(r))
}

/// Guard for `git push …` typed or clicked in the git panel: in a synced project, main
/// can't be pushed (only Ship does that) and nothing is pushed with --all/--mirror.
pub fn guard_manual(root: &Path, push_args: &[String]) -> Result<(), String> {
    if !is_synced(root) {
        return Ok(());
    }
    let git = Git::new(root, None, None);
    let prot = protected(&git);
    let msg = "This project syncs with GitHub: main changes only through Ship (⌘⇧D).".to_string();
    let mut positional = vec![];
    for a in push_args.iter().skip_while(|a| *a == "push") {
        if a == "--all" || a == "--mirror" {
            return Err(msg);
        }
        if !a.starts_with('-') {
            positional.push(a.clone());
        }
    }
    // positional[0] is the remote, the rest refspecs.
    let specs: Vec<String> = positional.iter().skip(1).cloned().collect();
    if specs.is_empty() {
        let cur = git.current_branch().unwrap_or_default();
        if prot.contains(&cur) {
            return Err(msg);
        }
        return Ok(());
    }
    for s in specs {
        let dst = s.split(':').next_back().unwrap_or(&s).trim_start_matches('+').to_string();
        let dst = if dst == "HEAD" { git.current_branch().unwrap_or_default() } else { dst };
        if targets_protected(&dst, &prot) {
            return Err(msg);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::gitx::testutil::{sh, Fixture};
    use super::*;

    #[test]
    fn auth_errors_are_not_offline() {
        let ssh = "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.";
        let https = "remote: Repository not found.\nfatal: repository 'https://github.com/x/y.git/' not found";
        let gone = "fatal: could not read Username for 'https://github.com': terminal prompts disabled";
        for e in [ssh, https, gone] {
            assert!(is_auth(e) && !is_offline(e), "{e}");
        }
        assert!(is_offline("ssh: Could not resolve hostname github.com\nfatal: Could not read from remote repository."));
    }

    #[test]
    fn main_needs_a_main_push() {
        let p = vec!["main".to_string(), "master".to_string()];
        assert!(check(&[Refspec::branch("main")], &p, None).is_err());
        assert!(check(&[Refspec::new("refs/heads/fleet/px-a", "refs/heads/master")], &p, None).is_err());
        assert!(check(&[Refspec::new("HEAD", "main")], &p, None).is_err());
        assert!(check(&[Refspec::branch("fleet/px-a"), Refspec::branch("fleet-state")], &p, None).is_ok());
        assert!(check(&[Refspec::new("+refs/heads/x", "refs/heads/x")], &p, None).is_err());
    }

    #[test]
    fn push_refuses_main_and_detects_offline() {
        let fx = Fixture::new("push");
        let a = fx.clone("a");
        let g = fx.git(&a);
        sh(&a, "echo x >> a.txt && git commit -qam x");
        assert!(matches!(push(&g, &[Refspec::branch("main")], &[], None), PushOutcome::Refused(_)));
        sh(&a, "git switch -q -c fleet/px-a");
        assert_eq!(push(&g, &[Refspec::branch("fleet/px-a")], &[], None), PushOutcome::Pushed);
        std::fs::rename(&fx.bare, fx.dir.join("gone.git")).unwrap();
        assert!(matches!(push(&g, &[Refspec::branch("fleet/px-a")], &[], None), PushOutcome::Offline(_)), "{:?}", push(&g, &[Refspec::branch("fleet/px-a")], &[], None));
        std::fs::rename(fx.dir.join("gone.git"), &fx.bare).unwrap();
    }

    #[test]
    fn rejection_is_reported_per_ref() {
        let fx = Fixture::new("reject");
        let a = fx.clone("a");
        let b = fx.clone("b");
        sh(&a, "git switch -q -c fleet/px-a && echo a >> a.txt && git commit -qam a && git push -q origin fleet/px-a");
        sh(&b, "git fetch -q && git switch -q -c fleet/px-a origin/main && echo b >> b.txt && git commit -qam b");
        match push(&fx.git(&b), &[Refspec::branch("fleet/px-a")], &[], None) {
            PushOutcome::Rejected(refs, _) => assert_eq!(refs, vec!["refs/heads/fleet/px-a"]),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn manual_guard_only_in_synced_projects() {
        let fx = Fixture::new("guard");
        let a = fx.clone("a");
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(guard_manual(&a, &args(&["push"])).is_ok(), "not synced");
        set_synced_roots(vec![a.clone()]);
        assert!(guard_manual(&a, &args(&["push"])).is_err(), "on main");
        assert!(guard_manual(&a, &args(&["push", "origin", "HEAD:main"])).is_err());
        assert!(guard_manual(&a, &args(&["push", "--all", "origin"])).is_err());
        sh(&a, "git switch -q -c fleet/x");
        assert!(guard_manual(&a, &args(&["push"])).is_ok());
        assert!(guard_manual(&a, &args(&["push", "-u", "origin", "fleet/x"])).is_ok());
        assert!(guard_manual(&a.join(".fleet/worktrees/x"), &args(&["push", "origin", "main"])).is_err(), "worktrees count");
        set_synced_roots(vec![]);
    }

    /// The acceptance rule "no code path pushes to main except Ship": every `push`
    /// invocation in the crate lives in this file, and the git panel's goes through guard_manual.
    #[test]
    fn nothing_else_pushes() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = vec![];
        let mut stack = vec![src.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().map(|x| x != "rs").unwrap_or(true) {
                    continue;
                }
                let rel = p.strip_prefix(&src).unwrap().to_string_lossy().to_string();
                if rel == "sync/pusher.rs" {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                // Production code only: stop at the test module.
                let prod = text.split("#[cfg(test)]").next().unwrap_or("");
                for (i, line) in prod.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    let compares = code.contains("== \"push\"") || code.contains("!= \"push\"") || code.contains("\"stash\", \"push\"") || line.contains("// workflow-event");
                    if code.contains("\"push\"") && !compares {
                        offenders.push(format!("{rel}:{}", i + 1));
                    }
                }
            }
        }
        // git_cmds.rs: the panel's push and git_run("push", …) — both must call guard_manual.
        let panel = std::fs::read_to_string(src.join("git_cmds.rs")).unwrap();
        assert!(panel.matches("pusher::guard_manual").count() >= 3, "git panel pushes (commit+push, remote op, git_run) must be guarded");
        let others: Vec<_> = offenders.iter().filter(|o| !o.starts_with("git_cmds.rs")).collect();
        assert!(others.is_empty(), "push outside pusher.rs: {others:?}");
    }
}

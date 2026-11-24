//! Ship (⌘⇧D): the only code path that moves main. In a temporary worktree at origin's
//! main, each chosen branch is merged in order (squash by default), checked, and skipped on
//! conflict or failing checks; main is then pushed fast-forward only (or through a PR when
//! protected), the deploy command runs at exactly that commit, and a tag + deploys.yml
//! entry record it.
use super::autocommit;
use super::gitx::Git;
use super::pusher::{self, Lease, PushOutcome, Refspec};
use super::state_branch::{self, DeployEntry};
use std::path::{Path, PathBuf};

/// Permission to push main. The private field means only this module can create one,
/// so nothing outside the Ship sequence can push main (enforced at compile time).
pub struct MainPush {
    _private: (),
}

impl MainPush {
    fn new() -> MainPush {
        MainPush { _private: () }
    }
}

/// The one push of main outside Ship: a brand-new GitHub repo's first branch. Allowed only
/// while GitHub has no branches at all, and the lease insists the branch still doesn't exist,
/// so it can never replace anything.
pub fn first_push(git: &Git, branch: &str) -> PushOutcome {
    match git.read(&["ls-remote", "--heads", "origin"]) {
        Ok(heads) if heads.trim().is_empty() => {}
        Ok(_) => return PushOutcome::Refused("The GitHub repo already has branches; main changes only through Ship.".into()),
        Err(e) => return PushOutcome::Failed(e.to_string()),
    }
    let token = MainPush::new();
    let dst = format!("refs/heads/{branch}");
    pusher::push(git, &[Refspec::new(&dst, &dst)], &[Lease { dst: dst.clone(), expect: String::new() }], Some(&token))
}

#[derive(Debug, Clone)]
pub struct ShipBranch {
    pub branch: String,
    pub title: String,
    pub glyph: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum BranchResult {
    Merged { commit: String },
    Conflict { files: Vec<String>, output: String },
    ChecksFailed { check: String, output: String },
    Empty,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShipOutcome {
    pub results: Vec<(String, BranchResult)>,
    pub main_commit: Option<String>,
    pub pushed: bool,
    pub tag: Option<String>,
    pub deploy_code: Option<i32>,
    pub deploy_output: Option<String>,
    pub error: Option<String>,
    /// Branches still open after this Ship (they need a rebase onto the new main).
    pub others: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub step: String,
    pub branch: Option<String>,
    pub ok: Option<bool>,
}

/// Runs a shell command in a folder (streaming its output somewhere); returns exit code + output tail.
pub type Runner<'a> = &'a dyn Fn(&Path, &str) -> (i32, String);

pub struct ShipInput<'a> {
    pub git: &'a Git,
    pub default_branch: String,
    pub deploy_dir: PathBuf,
    pub branches: Vec<ShipBranch>,
    pub prefix: String,
    pub checks: Vec<String>,
    pub setup: Option<String>,
    pub deploy_command: Option<String>,
    pub merge_style: String,
    pub machine: String,
    pub run: Runner<'a>,
    pub progress: &'a dyn Fn(Progress),
    /// Protected main: merge `head` (a pushed temp branch) through a PR and wait. Returns Err on failure.
    pub via_pr: Option<&'a dyn Fn(&Git, &str, &str) -> Result<(), String>>,
    /// Tag time (local), e.g. "20261123-1402".
    pub stamp: String,
}

fn step(inp: &ShipInput, s: &str, branch: Option<&str>, ok: Option<bool>) {
    (inp.progress)(Progress { step: s.into(), branch: branch.map(String::from), ok });
}

fn tail(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut cut = s.len() - n;
    while !s.is_char_boundary(cut) {
        cut += 1;
    }
    format!("…{}", &s[cut..])
}

fn prepare(inp: &ShipInput, base: &str) -> Result<Git, String> {
    let g = inp.git;
    let dir = inp.deploy_dir.to_string_lossy().to_string();
    if inp.deploy_dir.exists() {
        let _ = g.run(&["worktree", "remove", "--force", &dir]);
        let _ = std::fs::remove_dir_all(&inp.deploy_dir);
        let _ = g.read(&["worktree", "prune"]);
    }
    if let Some(parent) = inp.deploy_dir.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    g.run(&["worktree", "add", "-q", "--detach", &dir, base]).map_err(|e| e.to_string())?;
    let dg = g.at(&inp.deploy_dir);
    if let Some(setup) = inp.setup.as_deref().filter(|s| !s.trim().is_empty()) {
        step(inp, &format!("setup: {setup}"), None, None);
        let (code, out) = (inp.run)(&inp.deploy_dir, setup);
        if code != 0 {
            return Err(format!("Setup failed ({setup}):\n{}", tail(&out, 3000)));
        }
    }
    Ok(dg)
}

fn cleanup(inp: &ShipInput) {
    let dir = inp.deploy_dir.to_string_lossy().to_string();
    let _ = inp.git.run(&["worktree", "remove", "--force", &dir]);
    let _ = std::fs::remove_dir_all(&inp.deploy_dir);
    let _ = inp.git.read(&["worktree", "prune"]);
}

/// Merge one branch onto the deploy head; Ok(Some(new head)) | Ok(None) if empty | Err(result).
fn merge_one(inp: &ShipInput, dg: &Git, head: &str, b: &ShipBranch) -> Result<Option<String>, BranchResult> {
    let remote = format!("refs/remotes/origin/{}", b.branch);
    let reset = || {
        let _ = dg.read(&["merge", "--abort"]);
        let _ = dg.read(&["rebase", "--abort"]);
        let _ = dg.run(&["reset", "-q", "--hard", head]);
        let _ = dg.read(&["checkout", "-q", "--detach", head]);
    };
    let ident = dg.ident_env();
    let ident: Vec<(&str, &str)> = ident.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let merged = match inp.merge_style.as_str() {
        "rebase" => dg.run(&["checkout", "-q", "--detach", &remote]).and_then(|_| dg.run_with(&["rebase", head], &ident, None, true)),
        "merge" => dg.run_with(&["merge", "--no-ff", "--no-commit", &remote], &ident, None, true),
        _ => dg.run_with(&["merge", "--squash", "--no-commit", &remote], &ident, None, true),
    };
    if let Err(e) = merged {
        let files = autocommit::conflicted(dg);
        reset();
        return Err(BranchResult::Conflict { files, output: e.to_string() });
    }
    for check in &inp.checks {
        step(inp, &format!("check: {check}"), Some(&b.branch), None);
        let (code, out) = (inp.run)(&inp.deploy_dir, check);
        if code != 0 {
            reset();
            return Err(BranchResult::ChecksFailed { check: check.clone(), output: tail(&out, 6000) });
        }
    }
    let trailers = format!("Fleet-Session: {}\nFleet-Title: {}\nFleet-Branch: {}\nFleet-Machine: {}", b.glyph, b.title.replace('\n', " "), b.branch, inp.machine);
    match inp.merge_style.as_str() {
        "rebase" => {}
        "merge" => {
            let msg = format!("Merge {} ({})\n\n{trailers}\n", b.title, b.branch);
            dg.run_with(&["commit", "-q", "--no-verify", "-F", "-"], &ident, Some(msg.as_bytes()), true).map_err(|e| BranchResult::Conflict { files: vec![], output: e.to_string() })?;
        }
        _ => {
            if dg.read(&["diff", "--cached", "--quiet"]).is_ok() {
                reset();
                return Ok(None);
            }
            let subjects = dg.read(&["log", "--format=- %s", &format!("{head}..{remote}")]).unwrap_or_default();
            let msg = format!("{}\n\n{}\n{trailers}\n", b.title, subjects.trim_end());
            dg.run_with(&["commit", "-q", "--no-verify", "-F", "-"], &ident, Some(msg.as_bytes()), true).map_err(|e| BranchResult::Conflict { files: vec![], output: e.to_string() })?;
        }
    }
    Ok(dg.rev("HEAD"))
}

pub fn ship(inp: &ShipInput) -> ShipOutcome {
    let mut out = ShipOutcome::default();
    let g = inp.git;
    let main_ref = format!("refs/remotes/origin/{}", inp.default_branch);
    let mut dg = None;
    for attempt in 0..2 {
        out = ShipOutcome::default();
        step(inp, "fetch", None, None);
        if let Err(e) = g.run(&["fetch", "--prune", "origin"]) {
            out.error = Some(format!("Couldn't reach GitHub: {e}"));
            return out;
        }
        let Some(base) = g.rev(&main_ref) else {
            out.error = Some(format!("origin/{} doesn't exist", inp.default_branch));
            return out;
        };
        let d = match prepare(inp, &base) {
            Ok(d) => d,
            Err(e) => {
                cleanup(inp);
                out.error = Some(e);
                return out;
            }
        };
        let mut head = base.clone();
        for b in &inp.branches {
            step(inp, "merge", Some(&b.branch), None);
            if g.rev(&format!("refs/remotes/origin/{}", b.branch)).is_none() {
                out.results.push((b.branch.clone(), BranchResult::Conflict { files: vec![], output: format!("{} isn't on GitHub", b.branch) }));
                continue;
            }
            match merge_one(inp, &d, &head, b) {
                Ok(Some(h)) => {
                    head = h.clone();
                    step(inp, "merged", Some(&b.branch), Some(true));
                    out.results.push((b.branch.clone(), BranchResult::Merged { commit: h }));
                }
                Ok(None) => out.results.push((b.branch.clone(), BranchResult::Empty)),
                Err(r) => {
                    step(inp, "skipped", Some(&b.branch), Some(false));
                    out.results.push((b.branch.clone(), r));
                }
            }
        }
        if head == base {
            cleanup(inp);
            out.error = Some("Nothing to ship: no branch merged.".into());
            return out;
        }
        // main: fast-forward only, never forced.
        step(inp, "push main", None, None);
        if let Some(pr) = inp.via_pr {
            let tmp = format!("{}ship-{}", inp.prefix, inp.stamp);
            if let PushOutcome::Pushed = pusher::push(&d, &[Refspec::new(&head, &format!("refs/heads/{tmp}"))], &[], None) {
                let titles: Vec<String> = inp.branches.iter().map(|b| b.title.clone()).collect();
                if let Err(e) = pr(&d, &tmp, &titles.join(", ")) {
                    out.error = Some(e);
                    cleanup(inp);
                    return out;
                }
                let _ = g.run(&["fetch", "--prune", "origin"]);
                out.main_commit = g.rev(&main_ref);
                out.pushed = true;
            } else {
                out.error = Some("Couldn't push the ship branch for the pull request.".into());
                cleanup(inp);
                return out;
            }
            dg = Some(d);
            break;
        }
        let token = MainPush::new();
        match pusher::push(&d, &[Refspec::new(&head, &format!("refs/heads/{}", inp.default_branch))], &[Lease { dst: format!("refs/heads/{}", inp.default_branch), expect: base.clone() }], Some(&token)) {
            PushOutcome::Pushed => {
                out.main_commit = Some(head);
                out.pushed = true;
                dg = Some(d);
                break;
            }
            PushOutcome::Rejected(..) | PushOutcome::LeaseLost(..) if attempt == 0 => {
                step(inp, "main moved on GitHub — starting again", None, None);
                continue;
            }
            o => {
                cleanup(inp);
                out.error = Some(format!("GitHub didn't accept main: {o:?}"));
                return out;
            }
        }
    }
    let (Some(d), Some(main_commit)) = (dg, out.main_commit.clone()) else {
        cleanup(inp);
        out.error.get_or_insert_with(|| "main moved twice during Ship; try again.".into());
        return out;
    };
    // Deploy at exactly the pushed commit.
    let mut deployed_ok = true;
    if let Some(cmd) = inp.deploy_command.as_deref().filter(|c| !c.trim().is_empty()) {
        let _ = d.run(&["checkout", "-q", "--detach", &main_commit]);
        step(inp, &format!("deploy: {cmd}"), None, None);
        let (code, o) = (inp.run)(&inp.deploy_dir, cmd);
        out.deploy_code = Some(code);
        out.deploy_output = Some(tail(&o, 6000));
        deployed_ok = code == 0;
        step(inp, "deploy", None, Some(deployed_ok));
    }
    let kind = if inp.deploy_command.as_deref().map(|c| !c.trim().is_empty()).unwrap_or(false) { "deploy" } else { "ship" };
    if deployed_ok {
        let mut tag = format!("{kind}-{}", inp.stamp);
        if g.rev(&format!("refs/tags/{tag}")).is_some() {
            tag = format!("{tag}-{}", &main_commit[..6]);
        }
        if g.run(&["tag", &tag, &main_commit]).is_ok() {
            if let PushOutcome::Pushed = pusher::push(g, &[Refspec::new(&format!("refs/tags/{tag}"), &format!("refs/tags/{tag}"))], &[], None) {
                out.tag = Some(tag);
            }
        }
    }
    // Merged branches leave GitHub; fleet-state records the ship.
    let merged: Vec<String> = out.results.iter().filter(|(_, r)| matches!(r, BranchResult::Merged { .. } | BranchResult::Empty)).map(|(b, _)| b.clone()).collect();
    let deletes: Vec<Refspec> = merged.iter().map(|b| Refspec::delete(&format!("refs/heads/{b}"))).collect();
    if !deletes.is_empty() {
        let _ = pusher::push(g, &deletes, &[], None);
    }
    let _ = g.run(&["fetch", "--prune", "origin"]);
    let mut changes = vec![];
    for (path, mut f) in state_branch::read_sessions(g, state_branch::REMOTE) {
        if f.archived {
            continue;
        }
        if merged.contains(&f.branch) {
            f.archived = true;
        } else if g.rev(&format!("refs/remotes/origin/{}", f.branch)).is_some() {
            f.needs_rebase = true;
            out.others.push(f.branch.clone());
        } else {
            continue;
        }
        f.updated_at = state_branch::now_stamp();
        changes.push((path, Some(state_branch::to_yaml(&f))));
    }
    let mut log = state_branch::read_deploys(g, state_branch::REMOTE);
    log.push(DeployEntry {
        tag: out.tag.clone().unwrap_or_else(|| format!("{kind}-{}-failed", inp.stamp)),
        commit: main_commit.clone(),
        branches: merged.clone(),
        machine: inp.machine.clone(),
        at: state_branch::now_stamp(),
        result: match (kind, deployed_ok) {
            ("deploy", true) => "deployed".into(),
            ("deploy", false) => "deploy_failed".into(),
            _ => "shipped".into(),
        },
    });
    changes.push(("deploys.yml".into(), Some(state_branch::to_yaml(&log))));
    if state_branch::write(g, &changes, &format!("state: ship {} ({} branch(es))", out.tag.clone().unwrap_or_default(), merged.len())).is_ok() {
        super::load::push_state(g);
    }
    cleanup(inp);
    step(inp, "done", None, Some(true));
    out
}

/// Run a deploy command again at a tag (rollback = redeploy the previous one; main untouched).
pub fn redeploy(git: &Git, dir: &Path, tag: &str, setup: Option<&str>, cmd: &str, run: Runner) -> (i32, String) {
    let d = dir.to_string_lossy().to_string();
    let _ = git.run(&["worktree", "remove", "--force", &d]);
    let _ = std::fs::remove_dir_all(dir);
    if let Err(e) = git.run(&["worktree", "add", "-q", "--detach", &d, &format!("refs/tags/{tag}")]) {
        return (-1, e.to_string());
    }
    let mut res = (0, String::new());
    if let Some(s) = setup.filter(|s| !s.trim().is_empty()) {
        res = run(dir, s);
    }
    if res.0 == 0 {
        res = run(dir, cmd);
    }
    let _ = git.run(&["worktree", "remove", "--force", &d]);
    let _ = std::fs::remove_dir_all(dir);
    res
}

/// Undo a shipped feature: a revert commit on a new branch from main (it ships next time).
pub fn revert_branch(git: &Git, dir: &Path, default_branch: &str, commit: &str, branch: &str) -> Result<String, String> {
    let d = dir.to_string_lossy().to_string();
    let _ = git.run(&["worktree", "remove", "--force", &d]);
    git.run(&["worktree", "add", "-q", "-b", branch, &d, &format!("origin/{default_branch}")]).map_err(|e| e.to_string())?;
    let wg = git.at(dir);
    let ident = wg.ident_env();
    let ident: Vec<(&str, &str)> = ident.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut args = vec!["revert", "--no-edit"];
    // A merge commit (merge style) reverts against its first parent.
    if wg.read(&["rev-parse", "--verify", "-q", &format!("{commit}^2")]).is_ok() {
        args.extend(["-m", "1"]);
    }
    args.push(commit);
    let res = wg.run_with(&args, &ident, None, true);
    let head = wg.rev("HEAD");
    let _ = git.run(&["worktree", "remove", "--force", &d]);
    res.map_err(|e| e.to_string())?;
    head.ok_or_else(|| "revert produced no commit".into())
}

pub fn ship_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::super::gitx::testutil::{sh, Fixture};
    use super::super::state_branch::{to_yaml, write, SessionFile};
    use super::*;
    use std::cell::Cell;

    fn shell(dir: &Path, cmd: &str) -> (i32, String) {
        let o = std::process::Command::new("/bin/sh").arg("-c").arg(cmd).current_dir(dir).output().unwrap();
        (o.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
    }

    fn session(g: &Git, branch: &str, title: &str) {
        let f = SessionFile { branch: branch.into(), glyph: "XX".into(), color: "#fff".into(), title: title.into(), state: "done".into(), updated_at: "2026-11-23T10:00:00Z".into(), ..Default::default() };
        write(g, &[(state_branch::file_for(branch, "fleet/"), Some(to_yaml(&f)))], "state").unwrap();
    }

    fn input<'a>(g: &'a Git, root: &Path, branches: &[&str], run: Runner<'a>, progress: &'a dyn Fn(Progress)) -> ShipInput<'a> {
        ShipInput {
            git: g,
            default_branch: "main".into(),
            deploy_dir: root.join(".fleet/deploy"),
            branches: branches.iter().map(|b| ShipBranch { branch: b.to_string(), title: format!("Feature {b}"), glyph: "XX".into() }).collect(),
            prefix: "fleet/".into(),
            checks: vec!["test -f a.txt".into()],
            setup: None,
            deploy_command: Some("git rev-parse HEAD > ../deployed-at".into()),
            merge_style: "squash".into(),
            machine: "MacBook Pro".into(),
            run,
            progress,
            via_pr: None,
            stamp: "20261123-1402".into(),
        }
    }

    #[test]
    fn three_branches_one_conflicts() {
        let fx = Fixture::new("ship");
        let a = fx.clone("a");
        sh(&a, "echo .fleet/ >> .git/info/exclude");
        // Three feature branches from main; fleet/b conflicts with fleet/a on a.txt.
        sh(&a, "git switch -q -c fleet/a && echo A > a.txt && git commit -qam 'a: edit a' && echo A2 >> a.txt && git commit -qam 'a: more' && git push -q origin fleet/a && git switch -q main");
        sh(&a, "git switch -q -c fleet/b && echo B > a.txt && git commit -qam 'b: edit a' && git push -q origin fleet/b && git switch -q main");
        sh(&a, "git switch -q -c fleet/c && echo C > c.txt && git add c.txt && git commit -qm 'c: add c' && git push -q origin fleet/c && git switch -q main");
        sh(&a, "git switch -q -c fleet/d && echo D > d.txt && git add d.txt && git commit -qm 'd: open' && git push -q origin fleet/d && git switch -q main");
        let g = fx.git(&a);
        for b in ["fleet/a", "fleet/b", "fleet/c", "fleet/d"] {
            session(&g, b, b);
        }
        sh(&a, "git push -q origin fleet-state");
        let steps = std::cell::RefCell::new(vec![]);
        let progress = |p: Progress| steps.borrow_mut().push(p.step);
        let run = |d: &Path, c: &str| shell(d, c);
        let out = ship(&input(&g, &a, &["fleet/a", "fleet/b", "fleet/c"], &run, &progress));
        assert!(out.error.is_none(), "{out:?}");
        assert!(matches!(out.results[0].1, BranchResult::Merged { .. }));
        assert!(matches!(&out.results[1].1, BranchResult::Conflict { files, .. } if files == &vec!["a.txt".to_string()]), "{:?}", out.results[1]);
        assert!(matches!(out.results[2].1, BranchResult::Merged { .. }));
        let bare = fx.git(&fx.bare);
        // main = 2 squash commits on top, fast-forward of the old main.
        let log = bare.read(&["log", "--format=%s", "main"]).unwrap();
        assert_eq!(log, "Feature fleet/c\nFeature fleet/a\ninit\n");
        let msg = bare.read(&["log", "-1", "--format=%B", "main~1"]).unwrap();
        assert!(msg.contains("- a: more") && msg.contains("Fleet-Branch: fleet/a"), "{msg}");
        // Deploy ran at the pushed commit; tag pushed; deploys.yml entry.
        let deployed = std::fs::read_to_string(a.join(".fleet/deployed-at")).unwrap();
        assert_eq!(deployed.trim(), bare.rev("main").unwrap());
        assert_eq!(out.tag.as_deref(), Some("deploy-20261123-1402"));
        assert_eq!(bare.rev("refs/tags/deploy-20261123-1402"), bare.rev("main"));
        let log = state_branch::read_deploys(&bare, "fleet-state");
        assert_eq!(log.len(), 1);
        assert_eq!((log[0].result.as_str(), log[0].branches.clone()), ("deployed", vec!["fleet/a".to_string(), "fleet/c".to_string()]));
        // Merged branches gone + archived; the conflicting and the unselected one need a rebase.
        assert!(bare.rev("refs/heads/fleet/a").is_none() && bare.rev("refs/heads/fleet/c").is_none());
        assert!(bare.rev("refs/heads/fleet/b").is_some());
        let files: std::collections::HashMap<String, SessionFile> = state_branch::read_sessions(&bare, "fleet-state").into_iter().map(|(_, f)| (f.branch.clone(), f)).collect();
        assert!(files["fleet/a"].archived && files["fleet/c"].archived);
        assert!(!files["fleet/b"].archived && files["fleet/b"].needs_rebase);
        assert!(files["fleet/d"].needs_rebase);
        let mut others = out.others.clone();
        others.sort();
        assert_eq!(others, vec!["fleet/b", "fleet/d"]);
        assert!(!a.join(".fleet/deploy").exists(), "deploy worktree removed");
        assert!(steps.borrow().iter().any(|s| s == "check: test -f a.txt"));
    }

    #[test]
    fn failing_checks_skip_and_main_moving_restarts_once() {
        let fx = Fixture::new("ship2");
        let a = fx.clone("a");
        let other = fx.clone("other");
        sh(&a, "echo .fleet/ >> .git/info/exclude");
        sh(&a, "git switch -q -c fleet/ok && echo ok > ok.txt && git add ok.txt && git commit -qm ok && git push -q origin fleet/ok && git switch -q main");
        sh(&a, "git switch -q -c fleet/bad && echo bad > bad.txt && git add bad.txt && git commit -qm bad && git push -q origin fleet/bad && git switch -q main");
        let g = fx.git(&a);
        // While the first check runs, someone else's ship moves main.
        let moved = Cell::new(false);
        let run = |d: &Path, c: &str| {
            if !moved.get() {
                moved.set(true);
                sh(&other, "echo hotfix > hot.txt && git add hot.txt && git commit -qm hotfix && git push -q origin main");
            }
            shell(d, c)
        };
        let progress = |_: Progress| {};
        let mut inp = input(&g, &a, &["fleet/ok", "fleet/bad"], &run, &progress);
        inp.checks = vec!["test ! -f bad.txt".into()];
        inp.deploy_command = None;
        let out = ship(&inp);
        assert!(out.error.is_none(), "{out:?}");
        assert!(matches!(out.results[0].1, BranchResult::Merged { .. }));
        assert!(matches!(&out.results[1].1, BranchResult::ChecksFailed { check, .. } if check == "test ! -f bad.txt"));
        let log = fx.git(&fx.bare).read(&["log", "--format=%s", "main"]).unwrap();
        assert_eq!(log, "Feature fleet/ok\nhotfix\ninit\n", "rebuilt on the moved main, never forced");
        assert_eq!(out.tag.as_deref(), Some("ship-20261123-1402"));
    }

    #[test]
    fn revert_goes_on_a_new_branch() {
        let fx = Fixture::new("revert");
        let a = fx.clone("a");
        sh(&a, "echo x > x.txt && git add x.txt && git commit -qm 'Feature X' && git push -q origin main && git fetch -q");
        let g = fx.git(&a);
        let feat = g.rev("HEAD").unwrap();
        let head = revert_branch(&g, &a.join(".fleet/revert"), "main", &feat, "fleet/revert-feature-x").unwrap();
        assert!(!a.join(".fleet/revert").exists());
        let msg = g.read(&["log", "-1", "--format=%s", &head]).unwrap();
        assert!(msg.starts_with("Revert \"Feature X\""), "{msg}");
        assert_eq!(g.rev("refs/heads/main"), Some(feat), "main untouched");
    }
}

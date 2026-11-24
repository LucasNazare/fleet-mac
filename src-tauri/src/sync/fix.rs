//! "Fix with Claude": a branch that conflicts with main is handed to a claude session — its
//! own session (prompted if open, resumed if closed), else a new one on a worktree of it.
//!
//! The branch takes main in with a merge (one resolution, and the normal push after the
//! prompt is a fast-forward). Only when Ship rebases (`mergeStyle: rebase`) does the branch
//! have to be rebased; its push then replaces GitHub's copy under a lease (`SyncState.rewrite`).
use super::*;
use presence::Holder;

/// What's already going on in the worktree, as far as the prompt cares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    None,
    Merge,
    RebaseOntoMain,
}

/// The message claude gets. `diverged`: GitHub's copy of the branch has commits this one lacks.
pub fn prompt(why: &str, branch: &str, main: &str, rebase: bool, op: Op, diverged: bool) -> String {
    let mut steps: Vec<String> = vec![];
    match op {
        Op::Merge => steps.push("A merge is already in progress here (`git status` shows it). Finish it first: resolve the conflicted files, `git add` them and `git commit --no-edit`.".into()),
        Op::RebaseOntoMain => steps.push(format!("A rebase onto origin/{main} is already in progress here (`git status` shows it). Resolve each conflicted file, `git add` it and run `GIT_EDITOR=true git rebase --continue`, until the rebase is done.")),
        Op::None => {}
    }
    if diverged && op != Op::RebaseOntoMain {
        steps.push(format!("GitHub's copy of this branch has commits this one doesn't: `git merge origin/{branch}` first (resolve and commit if it conflicts)."));
    }
    if op != Op::RebaseOntoMain {
        if rebase {
            steps.push(format!("`git fetch origin {main}` then `git rebase origin/{main}`. For each stop: resolve the conflicted files, `git add` them, `GIT_EDITOR=true git rebase --continue`."));
        } else {
            steps.push(format!("`git fetch origin {main}` then `git merge origin/{main}`. Resolve every conflicted file, `git add` them and `git commit --no-edit`."));
        }
    }
    steps.push(format!("While resolving: {main} is what's live in production, so keep its changes and re-apply this branch's change on top of them. Remove every conflict marker."));
    steps.push("Run the project's checks/tests if it has any, and fix what the merge broke.".into());
    let list = steps.iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect::<Vec<_>>().join("\n");
    format!(
        "{why}\n\nPlease fix the conflict on this branch ({branch}):\n{list}\n\nDon't push and don't touch {main}: when you're done fleet pushes this branch, and it can be shipped again. If a conflict needs a product decision, stop and ask me instead of guessing. End with a short summary of how you resolved each file."
    )
}

/// Get the worktree ready for claude. A rebase left behind (by sync or after a Ship) is
/// aborted unless Ship rebases and it's a rebase onto main; returns what's still in progress.
pub fn prepare(git: &Git, main: &str, rebase: bool) -> Op {
    match git.operation_in_progress() {
        Some("merge") => Op::Merge,
        Some("rebase") => {
            let onto = ["rebase-merge/onto", "rebase-apply/onto"].iter().find_map(|f| std::fs::read_to_string(git.git_dir().join(f)).ok()).unwrap_or_default();
            if rebase && git.rev(&format!("refs/remotes/origin/{main}")).as_deref() == Some(onto.trim()) {
                Op::RebaseOntoMain
            } else {
                let _ = git.run(&["rebase", "--abort"]);
                Op::None
            }
        }
        Some(_) => {
            let _ = git.run(&["cherry-pick", "--abort"]);
            Op::None
        }
        None => Op::None,
    }
}

/// GitHub's copy has commits the local branch lacks.
fn diverged(git: &Git, branch: &str) -> bool {
    let r = format!("refs/remotes/origin/{branch}");
    git.rev(&r).is_some() && !git.is_ancestor(&r, "HEAD")
}

fn fetch(git: &Git, main: &str, branch: &str) {
    let _ = git.run(&["fetch", "-q", "origin", &format!("+refs/heads/{0}:refs/remotes/origin/{0}", main), &format!("+refs/heads/{0}:refs/remotes/origin/{0}", branch)]);
}

/// Paste a prompt into a running session and submit it.
fn submit(app: &AppHandle, id: &str, text: &str) {
    let f = app.state::<Fleet>();
    let data = format!("\x1b[200~{}\x1b[201~", text.replace("\x1b[201~", ""));
    let _ = f.ptys.write(id, data.as_bytes());
    std::thread::sleep(Duration::from_millis(150));
    let _ = f.ptys.write(id, b"\r");
}

fn mark(app: &AppHandle, id: &str, detail: String) {
    set_sync(app, id, |i| {
        i.badge = "conflict".into();
        i.detail = Some(detail);
    });
}

/// Ship skipped a branch for a conflict: its open, idle session starts fixing it right away;
/// otherwise the session is marked so "Fix with Claude" is one click.
pub fn after_ship_conflict(app: &AppHandle, s: &Session, main: &str, rebase: bool, files: &[String]) {
    let why = format!("Ship skipped this branch: it conflicts with {main}{}.", if files.is_empty() { String::new() } else { format!(" in {}", files.join(", ")) });
    crate::update_session(app, &s.id, |x| {
        x.state = SessionState::NeedsReply;
        x.state_since = now_ms();
        x.pending_question = Some(why.clone());
        true
    });
    mark(app, &s.id, format!("Conflicts with {main}{}", if files.is_empty() { String::new() } else { format!(" in {}", files.join(", ")) }));
    let Some(wt) = s.worktree_path.clone().filter(|w| Path::new(w).is_dir()) else { return };
    if !s.alive || s.state == SessionState::Working {
        return;
    }
    let git = git_for(app, &wt);
    let op = prepare(&git, main, rebase);
    remember_rewrite(app, &s.id, &git, &s.branch, rebase);
    submit(app, &s.id, &prompt(&why, &s.branch, main, rebase, op, diverged(&git, &s.branch)));
}

fn remember_rewrite(app: &AppHandle, id: &str, git: &Git, branch: &str, rebase: bool) {
    let st = app.state::<SyncState>();
    match git.rev(&format!("refs/remotes/origin/{branch}")).filter(|_| rebase) {
        Some(sha) => st.rewrite.lock().insert(id.to_string(), sha),
        None => st.rewrite.lock().remove(id),
    };
}

/// "Fix with Claude" on a conflicting branch. Returns the session doing it.
#[tauri::command]
pub async fn sync_fix_conflict(app: AppHandle, project_id: String, branch: String, why: Option<String>, cols: u16, rows: u16) -> Result<Session, String> {
    let p = project(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || fix(&app, &p, &branch, why, cols, rows)).await.map_err(|e| e.to_string())?
}

fn fix(app: &AppHandle, p: &Project, branch: &str, why: Option<String>, cols: u16, rows: u16) -> Result<Session, String> {
    let c = cfg(app, &p.id).filter(|c| c.enabled).ok_or("GitHub sync isn't on for this project.")?;
    let conf = c.config.clone().unwrap_or_else(|| fleetyml::parse("project: x\n").unwrap());
    let main = c.default_branch.clone();
    let rebase = conf.sync.merge_style == "rebase";
    if branch.is_empty() || branch == main {
        return Err("Only a session's branch can be fixed this way.".into());
    }
    let why = why.filter(|w| !w.trim().is_empty()).unwrap_or_else(|| format!("This branch conflicts with {main}."));
    // Its session on this Mac: open ones first, then archived/closed; it knows why it made
    // its changes, so it resolves them best.
    let local = {
        let f = app.state::<Fleet>();
        let store = f.store.lock();
        let mut v: Vec<Session> = store.sessions.iter().filter(|s| s.project_id == p.id && s.branch == branch && s.remote.is_none()).cloned().collect();
        v.sort_by_key(|s| (!s.alive, s.archived));
        v.into_iter().find(|s| s.worktree_path.as_deref().is_some_and(|w| Path::new(w).is_dir()))
    };
    if let Some(s) = local {
        if s.alive && s.state == SessionState::Working {
            return Err(format!("{} · {} is working right now. Let it finish, then try again.", s.glyph, s.title));
        }
        let git = git_for(app, s.worktree_path.clone().unwrap());
        fetch(&git, &main, branch);
        let op = {
            let lock = session_lock(app, &s.id);
            let _g = lock.lock();
            prepare(&git, &main, rebase)
        };
        remember_rewrite(app, &s.id, &git, branch, rebase);
        let text = prompt(&why, branch, &main, rebase, op, diverged(&git, branch));
        mark(app, &s.id, "Claude is fixing the conflict".into());
        if s.alive {
            submit(app, &s.id, &text);
            crate::update_session(app, &s.id, |x| {
                x.state = SessionState::Working;
                x.state_since = now_ms();
                x.pending_question = None;
                true
            });
            return find(app, &s.id).map(|x| x.0).ok_or_else(|| "unknown session".into());
        }
        return crate::resume_session(app, &s.id, Some(&text), cols, rows);
    }
    // No session here: a new one on a worktree of the branch, with the identity from fleet-state.
    let root = PathBuf::from(&p.root_path);
    let git = git_for(app, &root);
    git.run(&["fetch", "-q", "origin", &format!("+refs/heads/{0}:refs/remotes/origin/{0}", main), &format!("+refs/heads/{0}:refs/remotes/origin/{0}", branch), "+refs/heads/fleet-state:refs/remotes/origin/fleet-state"])
        .map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    if let Holder::Other { machine, age_ms } = presence::holder(presence::fetch_one(&git, branch).as_ref(), &load::install_id(), now_ms()) {
        return Err(format!("{branch} is open on {machine} ({}). Fix it there, or Continue here first.", presence::ago(age_ms)));
    }
    let path = state_branch::file_for(branch, &conf.sync.branch_prefix);
    let file = state_branch::read_session(&git, state_branch::REMOTE, &path);
    let stem = path.trim_start_matches("sessions/").trim_end_matches(".yml").to_string();
    let wt = load::worktree_on_branch(&git, &root, branch, &stem)?;
    let wgit = git.at(&wt);
    if let Some(setup) = conf.sync.setup.clone().filter(|s| !s.trim().is_empty()) {
        run_setup(app, &wt, &setup);
    }
    let op = prepare(&wgit, &main, rebase);
    let text = prompt(&why, branch, &main, rebase, op, diverged(&wgit, branch));
    let s = crate::create_session(
        app,
        p,
        crate::SessionSpec {
            title: file.as_ref().map(|f| f.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| format!("Fix conflict: {branch}")),
            prompt: Some(text),
            worktree_path: Some(wt.clone()),
            branch: branch.to_string(),
            glyph: file.as_ref().map(|f| f.glyph.clone()).filter(|g| !g.is_empty()),
            color: file.as_ref().map(|f| f.color.clone()).filter(|c| !c.is_empty()),
            title_history: file.as_ref().map(|f| f.title_history.clone()).unwrap_or_default(),
            recap: file.as_ref().map(|f| f.recap.clone()).unwrap_or_default(),
            prompt_count: file.as_ref().map(|f| f.prompts).unwrap_or(0),
            sync: Some(SyncInfo { badge: "conflict".into(), detail: Some("Claude is fixing the conflict".into()), ..Default::default() }),
            ..Default::default()
        },
        cols,
        rows,
    )?;
    remember_rewrite(app, &s.id, &wgit, branch, rebase);
    // Claim the branch so another Mac doesn't push to it meanwhile.
    let ctx = autocommit::PresenceCtx { machine: machine_name(), install: load::install_id(), expect: None, take: true, rewritten_from: None };
    let (_, tag) = autocommit::push_presence(&wgit, branch, &ctx);
    load::remember_presence(app, &s.id, tag);
    app.state::<SyncState>().others.lock().entry(p.id.clone()).or_default().retain(|o| o.branch != branch);
    load::emit_others(app);
    Ok(find(app, &s.id).map(|x| x.0).unwrap_or(s))
}

#[cfg(test)]
mod tests {
    use super::super::gitx::testutil::{sh, Fixture};
    use super::*;

    #[test]
    fn prompt_merges_by_default_and_rebases_only_when_ship_does() {
        let m = prompt("Ship skipped this branch.", "fleet/px-pix", "main", false, Op::None, false);
        assert!(m.starts_with("Ship skipped this branch."));
        assert!(m.contains("git merge origin/main") && !m.contains("git rebase origin/main"), "{m}");
        assert!(m.contains("Don't push"));
        let r = prompt("x", "fleet/px-pix", "main", true, Op::None, true);
        assert!(r.contains("git merge origin/fleet/px-pix") && r.contains("git rebase origin/main"), "{r}");
        let cont = prompt("x", "fleet/px-pix", "main", true, Op::RebaseOntoMain, true);
        assert!(cont.contains("rebase --continue") && !cont.contains("git merge origin/fleet/px-pix"), "{cont}");
    }

    /// The whole flow on real git: conflict with main → (claude's) merge → plain push → main
    /// takes the branch cleanly.
    #[test]
    fn merge_fix_pushes_without_rewriting() {
        let fx = Fixture::new("fix-merge");
        let a = fx.clone("a");
        sh(&a, "git switch -q -c fleet/px && echo branch > a.txt && git commit -qam branch && git push -q origin fleet/px");
        sh(&a, "git switch -q main && echo main > a.txt && git commit -qam main && git push -q origin main && git switch -q fleet/px");
        let g = fx.git(&a);
        assert_eq!(prepare(&g, "main", false), Op::None);
        let before = g.rev("refs/remotes/origin/fleet/px").unwrap();
        // What claude does with the prompt:
        assert!(g.run(&["merge", "origin/main"]).is_err(), "conflicts");
        assert_eq!(prepare(&g, "main", false), Op::Merge, "a merge in progress is left for claude");
        sh(&a, "echo both > a.txt && git add a.txt && git commit -q --no-edit");
        assert!(g.is_ancestor(&before, "HEAD"), "no rewrite: the push is a fast-forward");
        let (st, _) = autocommit::push_session(&g, "fleet/px", None);
        assert_eq!(st, autocommit::Status::Pushed);
        assert!(g.is_ancestor("refs/remotes/origin/main", "refs/heads/fleet/px"), "main is in the branch now");
    }

    /// Ship rebases: the rebased branch replaces GitHub's under a lease, and a stray rebase
    /// that isn't onto main is aborted first.
    #[test]
    fn rebase_fix_pushes_with_lease() {
        let fx = Fixture::new("fix-rebase");
        let a = fx.clone("a");
        sh(&a, "git switch -q -c fleet/px && echo branch > a.txt && git commit -qam branch && git push -q origin fleet/px");
        sh(&a, "git switch -q main && echo main > a.txt && git commit -qam main && git push -q origin main && git switch -q fleet/px");
        let g = fx.git(&a);
        // Left behind: a rebase onto main (as after a Ship) → kept for claude when Ship rebases…
        assert!(g.run(&["rebase", "origin/main"]).is_err());
        assert_eq!(prepare(&g, "main", true), Op::RebaseOntoMain);
        // …and aborted when Ship merges/squashes.
        assert_eq!(prepare(&g, "main", false), Op::None);
        assert!(g.operation_in_progress().is_none());
        let before = g.rev("refs/remotes/origin/fleet/px").unwrap();
        assert!(g.run(&["rebase", "origin/main"]).is_err());
        sh(&a, "echo both > a.txt && git add a.txt && GIT_EDITOR=true git rebase --continue");
        assert!(!g.is_ancestor(&before, "HEAD"), "rewritten");
        let ctx = |from: Option<String>| autocommit::PresenceCtx { machine: "MacBook Pro".into(), install: "A".into(), expect: None, take: true, rewritten_from: from };
        // Without the lease the push would be rejected (then rebased onto the old branch).
        let (st, _) = autocommit::push_session(&g, "fleet/px", Some(&ctx(Some(before.clone()))));
        assert_eq!(st, autocommit::Status::Pushed, "{st:?}");
        assert_eq!(fx.git(&fx.bare).rev("fleet/px"), g.rev("HEAD"));
        // Someone else moved GitHub's copy meanwhile: the lease refuses instead of clobbering it.
        let b = fx.clone("b");
        sh(&b, "git fetch -q && git switch -q -c fleet/px origin/fleet/px && echo x > x.txt && git add x.txt && git commit -qm other && git push -q origin fleet/px");
        sh(&a, "git reset -q --hard HEAD~1 && echo again > a.txt && git commit -qam again");
        let (st, _) = autocommit::push_session(&g, "fleet/px", Some(&ctx(g.rev("refs/remotes/origin/fleet/px"))));
        assert!(matches!(st, autocommit::Status::Error(_)), "{st:?}");
    }
}

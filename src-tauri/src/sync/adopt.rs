//! "Keep local work as sessions": uncommitted changes (and local commits) in a synced
//! project's main folder move into session worktrees, so the folder matches GitHub again.
use super::gitx::{GResult, Git};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocalWork {
    /// Changed paths relative to the repo root (modified, added, deleted, untracked).
    pub files: Vec<String>,
    /// Commits on HEAD that origin's main doesn't have.
    pub ahead: u32,
    pub head: Option<String>,
    pub branch: Option<String>,
}

impl LocalWork {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.ahead == 0
    }
}

pub fn scan(git: &Git, default_branch: &str) -> LocalWork {
    let raw = git.read(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]).unwrap_or_default();
    let mut files = vec![];
    let mut it = raw.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = it.next() {
        if entry.len() < 4 {
            continue;
        }
        let (xy, path) = entry.split_at(3);
        if xy.starts_with('R') || xy.starts_with('C') {
            // "R  new\0old": both sides changed.
            if let Some(old) = it.next() {
                files.push(old.to_string());
            }
        }
        files.push(path.to_string());
    }
    files.sort();
    files.dedup();
    let origin = format!("refs/remotes/origin/{default_branch}");
    let ahead = if git.rev(&origin).is_some() && git.rev("HEAD").is_some() {
        git.read(&["rev-list", "--count", &format!("{origin}..HEAD")]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
    } else {
        0
    };
    LocalWork { files, ahead, head: git.rev("HEAD"), branch: git.current_branch() }
}

/// Stash everything (incl. untracked, not ignored). Returns the stash commit.
pub fn stash(git: &Git) -> GResult<Option<String>> {
    let before = git.rev("refs/stash");
    git.run(&["stash", "push", "--include-untracked", "-m", "fleet: keep local changes as sessions"])?;
    let after = git.rev("refs/stash");
    Ok(if after != before { after } else { None })
}

/// Recreate `files` from a stash commit in another checkout (same base).
pub fn materialize(wt: &Git, stash: &str, files: &[String]) -> GResult<()> {
    if files.is_empty() {
        return Ok(());
    }
    let tracked: BTreeMap<String, char> = wt
        .read(&["diff", "--name-status", "-z", "--no-renames", &format!("{stash}^1"), stash])
        .unwrap_or_default()
        .split('\0')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .chunks(2)
        .filter_map(|c| match c {
            [st, path] => Some((path.to_string(), st.chars().next().unwrap_or('M'))),
            _ => None,
        })
        .collect();
    let has_untracked = wt.rev(&format!("{stash}^3")).is_some();
    let untracked: BTreeSet<String> = if has_untracked {
        wt.read(&["ls-tree", "-r", "--name-only", "-z", &format!("{stash}^3")]).unwrap_or_default().split('\0').filter(|s| !s.is_empty()).map(String::from).collect()
    } else {
        BTreeSet::new()
    };
    for f in files {
        if untracked.contains(f) {
            wt.run(&["checkout", &format!("{stash}^3"), "--", f])?;
        } else {
            match tracked.get(f) {
                Some('D') => {
                    let _ = wt.run(&["rm", "-q", "--ignore-unmatch", "--", f]);
                }
                Some(_) => {
                    wt.run(&["checkout", stash, "--", f])?;
                }
                None => {}
            }
        }
    }
    let _ = wt.read(&["reset", "-q"]);
    Ok(())
}

/// Split changed files between sessions: a file touched by exactly one of them goes to it;
/// the rest stays unassigned ("Local changes").
pub fn assign(files: &[String], touched: &[(String, Vec<String>)]) -> (BTreeMap<String, Vec<String>>, Vec<String>) {
    let mut owned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut rest = vec![];
    for f in files {
        let owners: Vec<&String> = touched.iter().filter(|(_, t)| t.iter().any(|x| x == f)).map(|(id, _)| id).collect();
        if owners.len() == 1 {
            owned.entry(owners[0].clone()).or_default().push(f.clone());
        } else {
            rest.push(f.clone());
        }
    }
    (owned, rest)
}

/// Drop one stash entry by its sha. The stash list is shared by every worktree of the repo,
/// so never by position alone.
pub fn drop_stash(git: &Git, sha: &str) {
    let list = git.read(&["stash", "list", "--format=%H"]).unwrap_or_default();
    if let Some(i) = list.lines().position(|l| l.trim() == sha) {
        let _ = git.run(&["stash", "drop", "-q", &format!("stash@{{{i}}}")]);
    }
}

/// The folder is on another branch while local main has commits GitHub doesn't (and the
/// folder's HEAD doesn't carry them): resetting main would strand them. Their count.
pub fn stranded_main(git: &Git, work: &LocalWork, default_branch: &str) -> Option<u32> {
    if work.branch.as_deref() == Some(default_branch) {
        return None;
    }
    let local = git.rev(&format!("refs/heads/{default_branch}"))?;
    let origin = git.rev(&format!("refs/remotes/origin/{default_branch}"))?;
    if git.is_ancestor(&local, &origin) || git.is_ancestor(&local, "HEAD") {
        return None;
    }
    git.read(&["rev-list", "--count", &format!("{origin}..{local}")]).ok().and_then(|n| n.trim().parse().ok()).filter(|n| *n > 0)
}

/// Put the main folder back on origin's main once its work has moved out.
pub fn reset_main(git: &Git, work: &LocalWork, default_branch: &str) -> GResult<()> {
    let origin = format!("refs/remotes/origin/{default_branch}");
    if git.rev(&origin).is_none() {
        return Ok(());
    }
    if work.branch.as_deref() != Some(default_branch) {
        if git.rev(&format!("refs/heads/{default_branch}")).is_some() {
            git.run(&["switch", "-q", default_branch])?;
        } else {
            git.run(&["switch", "-q", "-c", default_branch, "--track", &format!("origin/{default_branch}")])?;
        }
    }
    let head = git.rev("HEAD");
    let o = git.rev(&origin);
    if head != o {
        match (&head, &o) {
            (Some(h), Some(r)) if git.is_ancestor(h, r) => {
                git.run(&["merge", "-q", "--ff-only", &origin])?;
            }
            _ => {
                // Local commits were carried into a session branch (it starts at the old HEAD).
                git.run(&["reset", "-q", "--keep", &origin])?;
            }
        }
    }
    Ok(())
}

/// Claude Code keeps a session's transcript under ~/.claude/projects/<cwd with non-alphanumerics as '-'>.
pub fn claude_project_dir(cwd: &str) -> std::path::PathBuf {
    let enc: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    dirs::home_dir().unwrap_or_default().join(".claude").join("projects").join(enc)
}

#[cfg(test)]
mod tests {
    use super::super::gitx::testutil::{sh, Fixture};
    use super::*;

    #[test]
    fn local_work_moves_into_session_worktrees() {
        let fx = Fixture::new("adopt");
        let a = fx.clone("a");
        let g = fx.git(&a);
        // A local commit + edits + a new file + a deletion.
        sh(&a, "echo local > c.txt && git add c.txt && git commit -qm 'local commit' && echo changed > a.txt && echo new > n.txt && git rm -q b.txt");
        let work = scan(&g, "main");
        assert_eq!(work.files, vec!["a.txt", "b.txt", "n.txt"]);
        assert_eq!(work.ahead, 1);
        let (owned, rest) = assign(&work.files, &[("s1".into(), vec!["a.txt".into(), "n.txt".into()]), ("s2".into(), vec!["n.txt".into()])]);
        assert_eq!(owned.get("s1").unwrap(), &vec!["a.txt".to_string()]);
        assert_eq!(rest, vec!["b.txt", "n.txt"], "n.txt has two owners");
        let st = stash(&g).unwrap().expect("stashed");
        let head = work.head.clone().unwrap();
        let (w1, w2) = (fx.dir.join("w1"), fx.dir.join("w2"));
        sh(&a, &format!("git worktree add -q --detach {} {head} && git worktree add -q --detach {} {head}", w1.display(), w2.display()));
        materialize(&g.at(&w1), &st, &owned["s1"]).unwrap();
        materialize(&g.at(&w2), &st, &rest).unwrap();
        assert_eq!(std::fs::read_to_string(w1.join("a.txt")).unwrap(), "changed\n");
        assert!(w1.join("b.txt").exists() && !w1.join("n.txt").exists());
        assert_eq!(std::fs::read_to_string(w2.join("a.txt")).unwrap(), "hello\n");
        assert!(!w2.join("b.txt").exists() && w2.join("n.txt").exists());
        assert!(w2.join("c.txt").exists(), "local commit carried");
        reset_main(&g, &work, "main").unwrap();
        assert!(scan(&g, "main").is_empty(), "main folder matches origin again");
        assert!(!a.join("c.txt").exists());
    }

    #[test]
    fn project_dir_encoding() {
        let d = claude_project_dir("/Users/x/Desktop/flat/fleet/.fleet/worktrees/a_b");
        assert!(d.ends_with(".claude/projects/-Users-x-Desktop-flat-fleet--fleet-worktrees-a-b"), "{d:?}");
    }

    #[test]
    fn stash_is_dropped_by_sha_not_position() {
        let fx = Fixture::new("stashdrop");
        let a = fx.clone("a");
        let g = fx.git(&a);
        sh(&a, "echo 1 > a.txt && git stash push -q -m mine");
        let mine = g.rev("refs/stash").unwrap();
        sh(&a, "echo 2 > a.txt && git stash push -q -m theirs");
        drop_stash(&g, &mine);
        let left = g.read(&["stash", "list", "--format=%s"]).unwrap();
        assert!(left.contains("theirs") && !left.contains("mine"), "{left}");
    }

    #[test]
    fn local_main_commits_are_not_stranded() {
        let fx = Fixture::new("stranded");
        let a = fx.clone("a");
        let g = fx.git(&a);
        sh(&a, "echo x > x.txt && git add x.txt && git commit -qm local && git switch -q -c side origin/main");
        let work = scan(&g, "main");
        assert_eq!(stranded_main(&g, &work, "main"), Some(1));
        sh(&a, "git switch -q main");
        assert_eq!(stranded_main(&g, &scan(&g, "main"), "main"), None);
    }
}

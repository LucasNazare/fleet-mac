//! Git via the git CLI: porcelain parsers, patch building for hunk/line staging,
//! and readable error messages. Pure functions here; commands live in git_cmds.rs.
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub orig_path: Option<String>,
    /// Index (staged) status letter, '.' = unchanged.
    pub index: char,
    /// Worktree status letter, '.' = unchanged.
    pub worktree: char,
    pub conflicted: bool,
    pub untracked: bool,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub staged_added: Option<u32>,
    pub staged_removed: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub branch: String,
    pub oid: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileEntry>,
}

/// `git status --porcelain=v2 --branch -z --untracked-files=all`
pub fn parse_status(text: &str) -> Status {
    let mut st = Status::default();
    let mut fields = text.split('\0').peekable();
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            let (k, v) = h.split_once(' ').unwrap_or((h, ""));
            match k {
                "branch.oid" => st.oid = v.to_string(),
                "branch.head" => st.branch = v.to_string(),
                "branch.upstream" => st.upstream = Some(v.to_string()),
                "branch.ab" => {
                    for part in v.split(' ') {
                        if let Some(a) = part.strip_prefix('+') {
                            st.ahead = a.parse().unwrap_or(0);
                        } else if let Some(b) = part.strip_prefix('-') {
                            st.behind = b.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = rec.chars().next().unwrap_or(' ');
        match kind {
            '1' | '2' | 'u' => {
                let n = match kind { '1' => 8, '2' => 9, _ => 10 };
                let parts: Vec<&str> = rec.splitn(n + 1, ' ').collect();
                if parts.len() < n + 1 {
                    continue;
                }
                let xy: Vec<char> = parts[1].chars().collect();
                let mut e = FileEntry {
                    path: parts[n].to_string(),
                    index: xy.first().copied().unwrap_or('.'),
                    worktree: xy.get(1).copied().unwrap_or('.'),
                    conflicted: kind == 'u',
                    ..Default::default()
                };
                if kind == '2' {
                    e.orig_path = fields.next().map(|s| s.to_string());
                }
                if kind == 'u' {
                    e.index = 'U';
                    e.worktree = 'U';
                }
                st.files.push(e);
            }
            '?' => st.files.push(FileEntry { path: rec[2..].to_string(), index: '.', worktree: '?', untracked: true, ..Default::default() }),
            _ => {}
        }
    }
    st
}

/// `git diff --numstat -z` → (path, added, removed). Binary files report None.
pub fn parse_numstat(text: &str) -> Vec<(String, Option<u32>, Option<u32>)> {
    let mut out = vec![];
    let mut it = text.split('\0');
    while let Some(rec) = it.next() {
        if rec.is_empty() {
            continue;
        }
        let mut p = rec.splitn(3, '\t');
        let (a, d, path) = (p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or(""));
        let path = if path.is_empty() {
            // rename: "\0old\0new"
            let _old = it.next();
            it.next().unwrap_or("").to_string()
        } else {
            path.to_string()
        };
        out.push((path, a.parse().ok(), d.parse().ok()));
    }
    out
}

pub fn apply_numstat(st: &mut Status, unstaged: &str, staged: &str) {
    for (path, a, d) in parse_numstat(unstaged) {
        if let Some(f) = st.files.iter_mut().find(|f| f.path == path) {
            f.added = a;
            f.removed = d;
        }
    }
    for (path, a, d) in parse_numstat(staged) {
        if let Some(f) = st.files.iter_mut().find(|f| f.path == path) {
            f.staged_added = a;
            f.staged_removed = d;
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub time: i64,
    pub subject: String,
    pub refs: String,
}

pub const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%at%x1f%D%x1f%s%x1e";

pub fn parse_log(text: &str) -> Vec<Commit> {
    text.split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            if rec.is_empty() {
                return None;
            }
            let f: Vec<&str> = rec.split('\x1f').collect();
            if f.len() < 6 {
                return None;
            }
            Some(Commit {
                hash: f[0].into(),
                parents: f[1].split(' ').filter(|s| !s.is_empty()).map(String::from).collect(),
                author: f[2].into(),
                time: f[3].parse().unwrap_or(0),
                refs: f[4].into(),
                subject: f[5].trim_end().into(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub full: String,
    pub remote: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub gone: bool,
    pub time: i64,
    pub head: bool,
    pub worktree: Option<String>,
}

pub const BRANCH_FORMAT: &str = "--format=%(refname)%1f%(refname:short)%1f%(upstream:short)%1f%(upstream:track)%1f%(committerdate:unix)%1f%(HEAD)%1f%(worktreepath)";

pub fn parse_branches(text: &str) -> Vec<Branch> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\x1f').collect();
            if f.len() < 7 || f[0].ends_with("/HEAD") {
                return None;
            }
            let track = f[3];
            let num = |key: &str| {
                track.find(key).and_then(|i| track[i + key.len()..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()).unwrap_or(0)
            };
            Some(Branch {
                full: f[0].into(),
                name: f[1].into(),
                remote: f[0].starts_with("refs/remotes/"),
                upstream: Some(f[2]).filter(|s| !s.is_empty()).map(String::from),
                ahead: num("ahead "),
                behind: num("behind "),
                gone: track.contains("gone"),
                time: f[4].parse().unwrap_or(0),
                head: f[5] == "*",
                worktree: Some(f[6]).filter(|s| !s.is_empty()).map(String::from),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub head: String,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
}

pub fn parse_worktrees(text: &str) -> Vec<Worktree> {
    text.split("\n\n")
        .filter_map(|block| {
            let mut w = Worktree { path: String::new(), head: String::new(), branch: None, bare: false, detached: false };
            for l in block.lines() {
                let (k, v) = l.split_once(' ').unwrap_or((l, ""));
                match k {
                    "worktree" => w.path = v.into(),
                    "HEAD" => w.head = v.into(),
                    "branch" => w.branch = Some(v.trim_start_matches("refs/heads/").into()),
                    "bare" => w.bare = true,
                    "detached" => w.detached = true,
                    _ => {}
                }
            }
            (!w.path.is_empty()).then_some(w)
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stash {
    pub name: String,
    pub message: String,
    pub time: i64,
}

pub fn parse_stashes(text: &str) -> Vec<Stash> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\x1f').collect();
            (f.len() >= 3).then(|| Stash { name: f[0].into(), message: f[1].into(), time: f[2].parse().unwrap_or(0) })
        })
        .collect()
}

// ------------------------------------------------------------------ hunks & patches

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub header: String,
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    /// Lines with their leading ' ', '+', '-' or '\\'.
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    /// Everything before the first hunk (diff --git, index, ---/+++ lines).
    pub header: String,
    pub hunks: Vec<Hunk>,
}

fn parse_range(s: &str) -> (u32, u32) {
    let s = &s[1..];
    match s.split_once(',') {
        Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
        None => (s.parse().unwrap_or(0), 1),
    }
}

/// Parse the unified diff of a single file.
pub fn parse_diff(text: &str) -> FileDiff {
    let mut d = FileDiff::default();
    for line in text.split_inclusive('\n') {
        let l = line.strip_suffix('\n').unwrap_or(line);
        if l.starts_with("@@ ") {
            let parts: Vec<&str> = l.split(' ').collect();
            let (os, ol) = parse_range(parts.get(1).unwrap_or(&"-0,0"));
            let (ns, nl) = parse_range(parts.get(2).unwrap_or(&"+0,0"));
            d.hunks.push(Hunk { header: l.into(), old_start: os, old_len: ol, new_start: ns, new_len: nl, lines: vec![] });
        } else if let Some(h) = d.hunks.last_mut() {
            h.lines.push(l.into());
        } else {
            d.header.push_str(line);
        }
    }
    d
}

/// Build a patch for one hunk, optionally limited to the new-file lines in
/// `select` (inclusive). `reverse` = the patch will be applied with --reverse
/// (unstage / discard), which flips how unselected lines are neutralised.
/// Apply with `git apply --recount`.
pub fn hunk_patch(diff: &FileDiff, hunk_index: usize, select: Option<(u32, u32)>, reverse: bool) -> Option<String> {
    let h = diff.hunks.get(hunk_index)?;
    let mut out_lines: Vec<String> = vec![];
    let mut new_no = h.new_start;
    let mut any_change = false;
    for l in &h.lines {
        let (kind, rest) = l.split_at(l.len().min(1));
        let selected = |n: u32| select.map(|(a, b)| n >= a && n <= b).unwrap_or(true);
        match kind {
            "+" => {
                let sel = selected(new_no);
                new_no += 1;
                if sel {
                    out_lines.push(l.clone());
                    any_change = true;
                } else if reverse {
                    out_lines.push(format!(" {rest}"));
                }
            }
            "-" => {
                // A deletion sits just before new line `new_no`.
                let sel = selected(new_no) || select.map(|(_, b)| new_no == b + 1).unwrap_or(false);
                if sel {
                    out_lines.push(l.clone());
                    any_change = true;
                } else if !reverse {
                    out_lines.push(format!(" {rest}"));
                }
            }
            "\\" => out_lines.push(l.clone()),
            _ => {
                new_no += 1;
                out_lines.push(l.clone());
            }
        }
    }
    if !any_change {
        return None;
    }
    let old_len = out_lines.iter().filter(|l| l.starts_with(' ') || l.starts_with('-')).count();
    let new_len = out_lines.iter().filter(|l| l.starts_with(' ') || l.starts_with('+')).count();
    let mut p = diff.header.clone();
    p.push_str(&format!("@@ -{},{} +{},{} @@\n", h.old_start, old_len, h.new_start, new_len));
    for l in out_lines {
        p.push_str(&l);
        p.push('\n');
    }
    Some(p)
}

// ------------------------------------------------------------------ errors

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitError {
    pub message: String,
    /// Machine name of a one-click fix the UI offers, e.g. "set_upstream_push".
    pub fix: Option<String>,
    pub fix_label: Option<String>,
    pub raw: String,
}

pub fn explain_error(stderr: &str, op: &str) -> GitError {
    let s = stderr.to_lowercase();
    let (message, fix, label): (String, Option<&str>, Option<&str>) = if s.contains("has no upstream branch") || s.contains("no upstream configured") || s.contains("there is no tracking information") {
        ("This branch has no upstream on the remote yet.".into(), Some("set_upstream_push"), Some("Set upstream and push"))
    } else if s.contains("non-fast-forward") || s.contains("fetch first") || (s.contains("rejected") && op == "push") {
        ("Push was rejected: the remote has commits you don't have. Pull first, then push.".into(), Some("pull"), Some("Pull"))
    } else if s.contains("conflict") || s.contains("fix conflicts") || s.contains("unmerged files") {
        ("There are merge conflicts. Resolve the files marked U, then commit.".into(), None, None)
    } else if s.contains("would be overwritten by") || s.contains("your local changes") {
        ("Your local changes would be overwritten. Commit or stash them first.".into(), Some("stash"), Some("Stash changes"))
    } else if s.contains("authentication failed") || s.contains("permission denied") || s.contains("could not read username") || s.contains("terminal prompts disabled") {
        ("Git couldn't authenticate with the remote. Check your credentials or SSH agent in a terminal.".into(), None, None)
    } else if s.contains("nothing to commit") {
        ("Nothing to commit — stage some changes first.".into(), None, None)
    } else if s.contains("not possible to fast-forward") || s.contains("divergent branches") {
        ("Your branch and the remote have diverged. Pull with rebase or merge.".into(), Some("pull_rebase"), Some("Pull with rebase"))
    } else if s.contains("could not resolve host") || s.contains("unable to access") {
        ("Couldn't reach the remote. Check your network.".into(), None, None)
    } else if s.contains("not a git repository") {
        ("This folder isn't a git repository.".into(), Some("init"), Some("git init"))
    } else {
        let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("git failed").trim();
        (first.trim_start_matches("error: ").trim_start_matches("fatal: ").to_string(), None, None)
    };
    GitError { message, fix: fix.map(String::from), fix_label: label.map(String::from), raw: stderr.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_v2() {
        let t = "# branch.oid abc123\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa bbb src/fee.ts\0\
1 A. N... 000000 100644 100644 000 ccc new file.ts\0\
2 R. N... 100644 100644 100644 ddd eee R100 src/new.ts\0src/old.ts\0\
u UU N... 100644 100644 100644 100644 f1 f2 f3 conflict.ts\0\
? notes.md\0";
        let s = parse_status(t);
        assert_eq!((s.branch.as_str(), s.ahead, s.behind), ("main", 2, 1));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!(s.files.len(), 5);
        assert_eq!((s.files[0].index, s.files[0].worktree), ('.', 'M'));
        assert_eq!(s.files[1].path, "new file.ts");
        assert_eq!(s.files[2].orig_path.as_deref(), Some("src/old.ts"));
        assert!(s.files[3].conflicted);
        assert!(s.files[4].untracked);
    }

    #[test]
    fn numstat() {
        let n = parse_numstat("3\t1\tsrc/a.ts\0-\t-\timg.png\0" );
        assert_eq!(n, vec![("src/a.ts".into(), Some(3), Some(1)), ("img.png".into(), None, None)]);
        let r = parse_numstat("1\t0\t\0old.ts\0new.ts\0");
        assert_eq!(r[0].0, "new.ts");
    }

    #[test]
    fn log_branches_worktrees_stash() {
        let log = parse_log("h1\x1fp1 p2\x1fAna\x1f1700000000\x1fHEAD -> main\x1fMerge x\x1e\nh2\x1f\x1fBo\x1f1600000000\x1f\x1fInit\x1e");
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].parents.len(), 2);
        assert!(log[1].parents.is_empty());
        let b = parse_branches("refs/heads/main\x1fmain\x1forigin/main\x1f[ahead 2, behind 3]\x1f1700\x1f*\x1f/r\nrefs/remotes/origin/HEAD\x1forigin\x1f\x1f\x1f0\x1f \x1f\nrefs/remotes/origin/main\x1forigin/main\x1f\x1f\x1f1700\x1f \x1f");
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].ahead, b[0].behind, b[0].head), (2, 3, true));
        assert!(b[1].remote);
        let w = parse_worktrees("worktree /r\nHEAD aaa\nbranch refs/heads/main\n\nworktree /r/.fleet/worktrees/x\nHEAD bbb\nbranch refs/heads/fleet/x\n");
        assert_eq!(w[1].branch.as_deref(), Some("fleet/x"));
        let s = parse_stashes("stash@{0}\x1fOn main: wip\x1f1700");
        assert_eq!(s[0].name, "stash@{0}");
    }

    const DIFF: &str = "diff --git a/f.txt b/f.txt\nindex 1..2 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,4 +1,5 @@\n a\n-b\n+B1\n+B2\n c\n d\n@@ -10,2 +11,3 @@\n x\n+y\n z\n";

    #[test]
    fn parses_hunks() {
        let d = parse_diff(DIFF);
        assert_eq!(d.hunks.len(), 2);
        assert!(d.header.starts_with("diff --git"));
        assert_eq!((d.hunks[1].old_start, d.hunks[1].new_start, d.hunks[1].new_len), (10, 11, 3));
    }

    #[test]
    fn whole_hunk_patch() {
        let d = parse_diff(DIFF);
        let p = hunk_patch(&d, 1, None, false).unwrap();
        assert!(p.ends_with("@@ -10,2 +11,3 @@\n x\n+y\n z\n"));
    }

    #[test]
    fn partial_line_patch() {
        let d = parse_diff(DIFF);
        // Only the new line 2 ("B1") and the deletion before it.
        let p = hunk_patch(&d, 0, Some((2, 2)), false).unwrap();
        assert!(p.contains("@@ -1,4 +1,4 @@\n a\n-b\n+B1\n c\n d\n"), "{p}");
        // Reverse (unstage/discard) keeps the unselected + as context and drops unselected -.
        let r = hunk_patch(&d, 0, Some((3, 3)), true).unwrap();
        assert!(r.contains(" a\n B1\n+B2\n c\n d\n"), "{r}");
        assert!(hunk_patch(&d, 1, Some((20, 30)), false).is_none());
    }

    #[test]
    fn error_messages() {
        let e = explain_error("fatal: The current branch feat has no upstream branch.\nTo push the current branch...", "push");
        assert_eq!(e.fix.as_deref(), Some("set_upstream_push"));
        let e = explain_error(" ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs", "push");
        assert_eq!(e.fix.as_deref(), Some("pull"));
        let e = explain_error("fatal: something odd", "fetch");
        assert_eq!(e.message, "something odd");
    }
}

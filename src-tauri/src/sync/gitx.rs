//! Git runner for automatic sync work. Actions (fetch, commit, push, update-ref…) are
//! logged so they show in the git panel's Output drawer; reads and plumbing internals are not.
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct LogLine {
    pub root: String,
    pub cmd: String,
    pub output: String,
    pub code: i32,
    pub ms: u64,
}

pub type Logger = Arc<dyn Fn(LogLine) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct GitFail {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl std::fmt::Display for GitFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.stderr.trim();
        if s.is_empty() {
            write!(f, "git exited with code {}", self.code)
        } else {
            write!(f, "{s}")
        }
    }
}

pub type GResult<T> = Result<T, GitFail>;

#[derive(Clone)]
pub struct Git {
    pub root: PathBuf,
    log: Option<Logger>,
    env: Option<Arc<HashMap<String, String>>>,
}

pub const ZERO: &str = "0000000000000000000000000000000000000000";

impl Git {
    pub fn new(root: impl Into<PathBuf>, log: Option<Logger>, env: Option<Arc<HashMap<String, String>>>) -> Self {
        Git { root: root.into(), log, env }
    }

    /// Same logger and environment, another checkout.
    pub fn at(&self, root: impl Into<PathBuf>) -> Git {
        Git { root: root.into(), log: self.log.clone(), env: self.env.clone() }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new("git");
        c.arg("-C").arg(&self.root);
        if let Some(env) = &self.env {
            c.env_clear().envs(env.iter());
        }
        c.env("GIT_TERMINAL_PROMPT", "0").env("LC_MESSAGES", "C").env("GIT_PAGER", "cat").env("GIT_EDITOR", "true");
        let has_ssh_cmd = self.env.as_ref().map(|e| e.contains_key("GIT_SSH_COMMAND")).unwrap_or_else(|| std::env::var_os("GIT_SSH_COMMAND").is_some());
        if !has_ssh_cmd {
            c.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=4");
        }
        // Fall back to a fleet identity only when the user has none configured.
        c
    }

    pub fn run_with(&self, args: &[&str], envs: &[(&str, &str)], stdin: Option<&[u8]>, logged: bool) -> GResult<String> {
        let t0 = Instant::now();
        let mut c = self.cmd();
        c.args(["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=30"]);
        c.args(args);
        for (k, v) in envs {
            c.env(k, v);
        }
        if !logged {
            c.env("GIT_OPTIONAL_LOCKS", "0");
        }
        c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| GitFail { code: -1, stdout: String::new(), stderr: e.to_string() })?;
        if let Some(input) = stdin {
            let _ = child.stdin.take().unwrap().write_all(input);
        }
        let out = child.wait_with_output().map_err(|e| GitFail { code: -1, stdout: String::new(), stderr: e.to_string() })?;
        let code = out.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if logged {
            if let Some(log) = &self.log {
                let mut output = format!("{stdout}{stderr}");
                if output.len() > 20_000 {
                    output.truncate(20_000);
                    output.push_str("\n… (truncated)");
                }
                log(LogLine {
                    root: self.root.to_string_lossy().into(),
                    cmd: format!("git {}", args.iter().map(|a| if a.contains(' ') || a.is_empty() { format!("'{a}'") } else { a.to_string() }).collect::<Vec<_>>().join(" ")),
                    output: output.trim_end().to_string(),
                    code,
                    ms: t0.elapsed().as_millis() as u64,
                });
            }
        }
        if code == 0 {
            Ok(stdout)
        } else {
            Err(GitFail { code, stdout, stderr })
        }
    }

    /// Logged action.
    pub fn run(&self, args: &[&str]) -> GResult<String> {
        self.run_with(args, &[], None, true)
    }

    /// Unlogged read.
    pub fn read(&self, args: &[&str]) -> GResult<String> {
        self.run_with(args, &[], None, false)
    }

    pub fn read_bytes(&self, args: &[&str]) -> Option<Vec<u8>> {
        let mut c = self.cmd();
        c.args(args).env("GIT_OPTIONAL_LOCKS", "0").stdin(Stdio::null());
        let out = c.output().ok()?;
        out.status.success().then_some(out.stdout)
    }

    /// Resolve a ref to a commit sha.
    pub fn rev(&self, r: &str) -> Option<String> {
        self.read(&["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.read(&["merge-base", "--is-ancestor", a, b]).is_ok()
    }

    pub fn current_branch(&self) -> Option<String> {
        self.read(&["symbolic-ref", "-q", "--short", "HEAD"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    pub fn git_dir(&self) -> PathBuf {
        let d = self.read(&["rev-parse", "--absolute-git-dir"]).map(|s| s.trim().to_string()).unwrap_or_default();
        PathBuf::from(d)
    }

    /// Rebase/merge/cherry-pick half done in this checkout.
    pub fn operation_in_progress(&self) -> Option<&'static str> {
        let d = self.git_dir();
        if d.join("rebase-merge").exists() || d.join("rebase-apply").exists() {
            Some("rebase")
        } else if d.join("MERGE_HEAD").exists() {
            Some("merge")
        } else if d.join("CHERRY_PICK_HEAD").exists() {
            Some("cherry-pick")
        } else {
            None
        }
    }

    pub fn has_remote(&self, name: &str) -> bool {
        self.read(&["remote"]).map(|r| r.lines().any(|l| l.trim() == name)).unwrap_or(false)
    }

    /// origin's default branch (refs/remotes/origin/HEAD), else main/master if present.
    pub fn default_branch(&self) -> String {
        if let Ok(s) = self.read(&["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"]) {
            if let Some(b) = s.trim().strip_prefix("origin/") {
                return b.to_string();
            }
        }
        for b in ["main", "master"] {
            if self.rev(&format!("refs/remotes/origin/{b}")).is_some() || self.rev(&format!("refs/heads/{b}")).is_some() {
                return b.to_string();
            }
        }
        "main".into()
    }

    /// Author/committer fallback when the user has no git identity configured.
    pub fn ident_env(&self) -> Vec<(&'static str, String)> {
        let name = self.read(&["config", "user.name"]).unwrap_or_default();
        let email = self.read(&["config", "user.email"]).unwrap_or_default();
        let mut v = vec![];
        if name.trim().is_empty() {
            v.push(("GIT_AUTHOR_NAME", "fleet".to_string()));
            v.push(("GIT_COMMITTER_NAME", "fleet".to_string()));
        }
        if email.trim().is_empty() {
            v.push(("GIT_AUTHOR_EMAIL", "fleet@localhost".to_string()));
            v.push(("GIT_COMMITTER_EMAIL", "fleet@localhost".to_string()));
        }
        v
    }

    pub fn show_file(&self, rev: &str, path: &str) -> Option<Vec<u8>> {
        self.read_bytes(&["show", &format!("{rev}:{path}")])
    }

    /// Commit `changes` (path → Some(content) | None to delete) on top of `base` without a checkout.
    /// Returns the new commit sha. `base` None makes a root commit from an empty tree.
    pub fn commit_files(&self, base: Option<&str>, changes: &[(String, Option<Vec<u8>>)], msg: &str) -> GResult<String> {
        let idx = std::env::temp_dir().join(format!("fleet-idx-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        let idx_s = idx.to_string_lossy().to_string();
        let env = [("GIT_INDEX_FILE", idx_s.as_str())];
        let res = (|| {
            match base {
                Some(b) => self.run_with(&["read-tree", b], &env, None, false)?,
                None => self.run_with(&["read-tree", "--empty"], &env, None, false)?,
            };
            for (path, content) in changes {
                match content {
                    Some(bytes) => {
                        let sha = self.run_with(&["hash-object", "-w", "--stdin"], &[], Some(bytes), false)?.trim().to_string();
                        self.run_with(&["update-index", "--add", "--cacheinfo", &format!("100644,{sha},{path}")], &env, None, false)?;
                    }
                    None => {
                        self.run_with(&["update-index", "--force-remove", "--", path], &env, None, false)?;
                    }
                }
            }
            let tree = self.run_with(&["write-tree"], &env, None, false)?.trim().to_string();
            let ident = self.ident_env();
            let ident_ref: Vec<(&str, &str)> = ident.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let mut args = vec!["commit-tree", tree.as_str()];
            if let Some(b) = base {
                args.push("-p");
                args.push(b);
            }
            Ok(self.run_with(&args, &ident_ref, Some(msg.as_bytes()), false)?.trim().to_string())
        })();
        let _ = std::fs::remove_file(&idx);
        res
    }

    /// Move a ref with compare-and-swap on its old value (logged).
    pub fn update_ref(&self, name: &str, new: &str, old: Option<&str>, reason: &str) -> GResult<()> {
        let old = old.unwrap_or(ZERO);
        self.run(&["update-ref", "-m", reason, name, new, old]).map(|_| ())
    }

    /// Files under `prefix` at `rev`: (path, content).
    pub fn read_tree_files(&self, rev: &str, prefix: &str) -> Vec<(String, Vec<u8>)> {
        let Ok(list) = self.read(&["ls-tree", "-r", "-z", "--full-tree", rev, "--", prefix]) else { return vec![] };
        let mut out = vec![];
        for entry in list.split('\0').filter(|e| !e.is_empty()) {
            // "<mode> blob <sha>\t<path>"
            let Some((meta, path)) = entry.split_once('\t') else { continue };
            let sha = meta.split_whitespace().nth(2).unwrap_or("");
            if let Some(b) = self.read_bytes(&["cat-file", "blob", sha]) {
                out.push((path.to_string(), b));
            }
        }
        out
    }
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

#[cfg(test)]
pub mod testutil {
    use super::*;
    use std::process::Command;

    /// A temp dir with a bare "GitHub" repo and one clone that has an initial commit on main.
    pub struct Fixture {
        pub dir: PathBuf,
        pub bare: PathBuf,
    }

    pub fn sh(dir: &Path, script: &str) -> String {
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "{script}\n{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    impl Fixture {
        pub fn new(name: &str) -> Fixture {
            let dir = std::env::temp_dir().join(format!("fleet-sync-{name}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let bare = dir.join("github.git");
            sh(&dir, "git init -q --bare -b main github.git && git clone -q github.git seed 2>/dev/null && cd seed && git checkout -q -b main && echo 'hello' > a.txt && echo 'one' > b.txt && git add . && git commit -qm init && git push -q origin main");
            Fixture { dir, bare }
        }

        /// A fresh clone ("another Mac").
        pub fn clone(&self, name: &str) -> PathBuf {
            sh(&self.dir, &format!("git clone -q github.git {name}"));
            let p = self.dir.join(name);
            sh(&p, "git config user.name t && git config user.email t@t");
            p
        }

        pub fn git(&self, p: &Path) -> Git {
            Git::new(p, None, None)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

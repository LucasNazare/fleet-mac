//! Project processes: dev servers, watchers and other long-running commands that
//! belong to a project rather than a session.
use crate::model::ProcessDef;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Exited,
    Crashed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRun {
    pub key: String,
    pub project_id: String,
    pub def_id: String,
    pub pid: Option<u32>,
    pub status: RunStatus,
    pub exit_code: Option<i32>,
    pub detected_ports: Vec<u16>,
    pub started_at: i64,
    /// Consecutive crash restarts (for backoff).
    pub restarts: u32,
    /// When an auto-restart is scheduled (unix ms).
    pub next_restart_at: Option<i64>,
    /// "stopping" while the INT → TERM → KILL sequence runs.
    #[serde(default)]
    pub stopping: bool,
    #[serde(skip)]
    pub restart_after_stop: bool,
    /// Left running by a previous fleet; no terminal attached.
    #[serde(default)]
    pub detached: bool,
    /// Runs in this session's worktree (synced projects), else the project root.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The session's glyph, shown as "dev · PX".
    #[serde(default)]
    pub base_label: Option<String>,
}

pub fn key(project_id: &str, def_id: &str) -> String {
    format!("proc:{project_id}:{def_id}")
}

/// 1 s, 2 s, 4 s … capped at 30 s.
pub fn backoff_secs(restarts: u32) -> u64 {
    (1u64 << restarts.min(5)).min(30)
}

// ------------------------------------------------------------------ .fleet/project.json

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFile {
    #[serde(default)]
    pub processes: Vec<ProcessDef>,
}

pub fn project_file(root: &Path) -> std::path::PathBuf {
    root.join(".fleet").join("project.json")
}

pub fn load_project_file(root: &Path) -> Option<ProjectFile> {
    let b = std::fs::read(project_file(root)).ok()?;
    serde_json::from_slice(&b).ok()
}

pub fn save_project_file(root: &Path, processes: &[ProcessDef]) -> std::io::Result<()> {
    let path = project_file(root);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let f = ProjectFile { processes: processes.to_vec() };
    std::fs::write(path, serde_json::to_vec_pretty(&f).unwrap())
}

/// Keep `.fleet/project.json` out of git via .git/info/exclude (not .gitignore, so
/// nothing shows up in the team's diff).
pub fn exclude_locally(root: &Path, pattern: &str) -> std::io::Result<bool> {
    let git = root.join(".git");
    if !git.is_dir() {
        return Ok(false); // worktrees / no repo: nothing to do
    }
    let path = git.join("info").join("exclude");
    std::fs::create_dir_all(path.parent().unwrap())?;
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == pattern) {
        return Ok(true);
    }
    let mut s = existing;
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(pattern);
    s.push('\n');
    std::fs::write(path, s)?;
    Ok(true)
}

// ------------------------------------------------------------------ detection

const SCRIPT_NAMES: &[&str] = &["dev", "start", "watch", "serve"];

pub(crate) fn package_runner(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if root.join("yarn.lock").exists() {
        "yarn"
    } else if root.join("bun.lockb").exists() || root.join("bun.lock").exists() {
        "bun"
    } else {
        "npm"
    }
}

pub(crate) fn run_script(runner: &str, script: &str) -> String {
    match (runner, script) {
        ("npm", "start") => "npm start".into(),
        ("npm", s) => format!("npm run {s}"),
        ("yarn", s) | ("pnpm", s) => format!("{runner} {s}"),
        (r, s) => format!("{r} run {s}"),
    }
}

fn def(id: &str, name: &str, command: String, cwd: &str) -> ProcessDef {
    ProcessDef { id: id.into(), name: name.into(), command, cwd: cwd.into(), ..Default::default() }
}

/// Suggested processes from package.json scripts, Procfile, docker compose and Makefile.
pub fn detect(root: &Path) -> Vec<ProcessDef> {
    let mut out = vec![];
    if let Some(pkg) = std::fs::read(root.join("package.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
        let runner = package_runner(root);
        if let Some(scripts) = pkg.get("scripts").and_then(|s| s.as_object()) {
            for name in SCRIPT_NAMES {
                if scripts.contains_key(*name) {
                    let cmd = run_script(runner, name);
                    out.push(def(&format!("pkg-{name}"), &cmd, cmd.clone(), "."));
                }
            }
        }
    }
    if let Ok(procfile) = std::fs::read_to_string(root.join("Procfile")) {
        for line in procfile.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if let Some((name, cmd)) = line.split_once(':') {
                let (name, cmd) = (name.trim(), cmd.trim());
                if !name.is_empty() && !cmd.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    out.push(def(&format!("procfile-{name}"), name, cmd.to_string(), "."));
                }
            }
        }
    }
    for f in ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"] {
        if root.join(f).exists() {
            out.push(def("compose", "docker compose up", "docker compose up".into(), "."));
            break;
        }
    }
    if let Ok(mk) = std::fs::read_to_string(root.join("Makefile")) {
        let mut seen = BTreeSet::new();
        for line in mk.lines() {
            let Some((target, _)) = line.split_once(':') else { continue };
            if line.starts_with(['\t', ' ', '.', '#']) || target.contains(['=', '$', ' ', '%']) || line.contains(":=") {
                continue;
            }
            if ["dev", "run", "serve", "watch", "start", "up"].contains(&target) && seen.insert(target.to_string()) {
                out.push(def(&format!("make-{target}"), &format!("make {target}"), format!("make {target}"), "."));
            }
        }
    }
    out
}

// ------------------------------------------------------------------ ports

/// Parse `ps -Ao pid=,ppid=` output.
pub fn parse_ps(text: &str) -> HashMap<u32, u32> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect()
}

/// Map every pid in `parents` (pid → ppid) to the root in `roots` it descends from.
pub fn owners(parents: &HashMap<u32, u32>, roots: &[u32]) -> HashMap<u32, u32> {
    let mut out = HashMap::new();
    for &pid in parents.keys().chain(roots.iter()) {
        let mut cur = pid;
        for _ in 0..64 {
            if roots.contains(&cur) {
                out.insert(pid, cur);
                break;
            }
            match parents.get(&cur) {
                Some(&p) if p > 1 && p != cur => cur = p,
                _ => break,
            }
        }
    }
    out
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN -Fpn` output into pid → ports.
pub fn parse_lsof(text: &str) -> HashMap<u32, BTreeSet<u16>> {
    let mut out: HashMap<u32, BTreeSet<u16>> = HashMap::new();
    let mut pid = 0u32;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().unwrap_or(0);
        } else if let Some(n) = line.strip_prefix('n') {
            if let Some(port) = n.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
                if pid != 0 {
                    out.entry(pid).or_default().insert(port);
                }
            }
        }
    }
    out
}

/// Listening TCP ports per root pid (including all descendants).
pub fn listening_ports(roots: &[u32]) -> HashMap<u32, Vec<u16>> {
    let mut result: HashMap<u32, Vec<u16>> = HashMap::new();
    if roots.is_empty() {
        return result;
    }
    let ps = std::process::Command::new("/bin/ps").args(["-Ao", "pid=,ppid="]).output();
    let Ok(ps) = ps else { return result };
    let parents = parse_ps(&String::from_utf8_lossy(&ps.stdout));
    let owner = owners(&parents, roots);
    let pids: Vec<String> = owner.keys().map(|p| p.to_string()).collect();
    if pids.is_empty() {
        return result;
    }
    let out = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &pids.join(","), "-Fpn"])
        .output();
    let Ok(out) = out else { return result };
    let mut per_root: HashMap<u32, BTreeSet<u16>> = HashMap::new();
    for (pid, ports) in parse_lsof(&String::from_utf8_lossy(&out.stdout)) {
        if let Some(root) = owner.get(&pid) {
            per_root.entry(*root).or_default().extend(ports);
        }
    }
    for (root, ports) in per_root {
        result.insert(root, ports.into_iter().collect());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_schedule() {
        let s: Vec<u64> = (0..8).map(backoff_secs).collect();
        assert_eq!(s, vec![1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn parses_lsof() {
        let t = "p100\nf20\nn*:5173\nf21\nn[::1]:5173\np200\nf9\nn127.0.0.1:3000\n";
        let m = parse_lsof(t);
        assert_eq!(m[&100].iter().copied().collect::<Vec<_>>(), vec![5173]);
        assert_eq!(m[&200].iter().copied().collect::<Vec<_>>(), vec![3000]);
    }

    #[test]
    fn process_tree_ownership() {
        // 10 (root sh) → 11 (npm) → 12 (node); 20 unrelated, child of 1.
        let parents = parse_ps("  10     5\n  11    10\n  12    11\n  20     1\n");
        let o = owners(&parents, &[10]);
        assert_eq!(o.get(&12), Some(&10));
        assert_eq!(o.get(&10), Some(&10));
        assert_eq!(o.get(&20), None);
    }

    #[test]
    fn detects_scripts_and_friends() {
        let dir = std::env::temp_dir().join(format!("fleet-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"dev":"vite","build":"vite build","start":"node ."}}"#).unwrap();
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(dir.join("Procfile"), "web: bundle exec puma\n# no\nworker: sidekiq\n").unwrap();
        std::fs::write(dir.join("compose.yaml"), "services: {}").unwrap();
        std::fs::write(dir.join("Makefile"), "VAR := 1\n.PHONY: dev\ndev: deps\n\tgo run .\nbuild:\n\tgo build\n").unwrap();
        let d = detect(&dir);
        let cmds: Vec<&str> = d.iter().map(|p| p.command.as_str()).collect();
        assert_eq!(cmds, vec!["pnpm dev", "pnpm start", "bundle exec puma", "sidekiq", "docker compose up", "make dev"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_file_roundtrip_and_exclude() {
        let dir = std::env::temp_dir().join(format!("fleet-pf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git/info")).unwrap();
        let d = vec![def("a", "dev", "npm run dev".into(), ".")];
        save_project_file(&dir, &d).unwrap();
        assert_eq!(load_project_file(&dir).unwrap().processes[0].command, "npm run dev");
        assert!(exclude_locally(&dir, ".fleet/project.json").unwrap());
        assert!(exclude_locally(&dir, ".fleet/project.json").unwrap());
        let ex = std::fs::read_to_string(dir.join(".git/info/exclude")).unwrap();
        assert_eq!(ex.matches(".fleet/project.json").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

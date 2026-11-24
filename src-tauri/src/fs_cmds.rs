//! File-system commands for the editor: tree, read/write, create/rename/trash,
//! quick open, find in files (ripgrep), replace all, format on save.
use crate::shell_env;
use serde::Serialize;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

type R<T> = Result<T, String>;
fn e<E: std::fmt::Display>(x: E) -> String {
    x.to_string()
}

const MAX_FILE: u64 = 8 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    name: String,
    path: String,
    is_dir: bool,
    ignored: bool,
}

/// One directory level. Ignored entries (per .gitignore) are marked, and omitted unless `show_ignored`.
#[tauri::command]
pub fn fs_list(dir: String, show_ignored: bool) -> R<Vec<Entry>> {
    let d = PathBuf::from(&dir);
    let visible: HashSet<PathBuf> = ignore::WalkBuilder::new(&d)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .parents(true)
        .require_git(false)
        .build()
        .filter_map(|r| r.ok())
        .map(|en| en.into_path())
        .collect();
    let mut out = vec![];
    for en in std::fs::read_dir(&d).map_err(e)?.flatten() {
        let name = en.file_name().to_string_lossy().to_string();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        let path = en.path();
        let ignored = !visible.contains(&path);
        if ignored && !show_ignored {
            continue;
        }
        let is_dir = en.file_type().map(|t| t.is_dir() || (t.is_symlink() && path.is_dir())).unwrap_or(false);
        out.push(Entry { name, path: path.to_string_lossy().into(), is_dir, ignored });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileText {
    text: String,
    mtime: u64,
    binary: bool,
    too_large: bool,
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[tauri::command]
pub fn fs_read(path: String) -> R<FileText> {
    let p = PathBuf::from(&path);
    let meta = std::fs::metadata(&p).map_err(e)?;
    if meta.len() > MAX_FILE {
        return Ok(FileText { text: String::new(), mtime: mtime(&p), binary: false, too_large: true });
    }
    let bytes = std::fs::read(&p).map_err(e)?;
    let binary = bytes.iter().take(8000).any(|b| *b == 0);
    Ok(FileText { text: if binary { String::new() } else { String::from_utf8_lossy(&bytes).into_owned() }, mtime: mtime(&p), binary, too_large: false })
}

#[tauri::command]
pub fn fs_write(path: String, text: String) -> R<u64> {
    let p = PathBuf::from(&path);
    let tmp = p.with_file_name(format!(".{}.fleet-tmp", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
    std::fs::write(&tmp, text.as_bytes()).map_err(e)?;
    if let Ok(meta) = std::fs::metadata(&p) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, &p).map_err(e)?;
    Ok(mtime(&p))
}

#[tauri::command]
pub fn fs_create(path: String, dir: bool) -> R<()> {
    let p = PathBuf::from(&path);
    if p.exists() {
        return Err(format!("{} already exists", p.display()));
    }
    if dir {
        std::fs::create_dir_all(&p).map_err(e)
    } else {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(e)?;
        }
        std::fs::write(&p, b"").map_err(e)
    }
}

#[tauri::command]
pub fn fs_rename(from: String, to: String) -> R<()> {
    if Path::new(&to).exists() {
        return Err(format!("{to} already exists"));
    }
    std::fs::rename(from, to).map_err(e)
}

#[tauri::command]
pub fn fs_trash(path: String) -> R<()> {
    trash::delete(&path).map_err(e)
}

#[tauri::command]
pub fn fs_reveal(path: String) -> R<()> {
    Command::new("/usr/bin/open").args(["-R", &path]).spawn().map_err(e)?;
    Ok(())
}

/// Tracked + untracked (not ignored) files, relative to `root`.
#[tauri::command]
pub fn quick_open_files(root: String) -> R<Vec<String>> {
    let out = Command::new("git")
        .args(["-C", &root, "ls-files", "-co", "--exclude-standard", "-z"])
        .envs(shell_env::login_env())
        .output();
    if let Ok(o) = out {
        if o.status.success() {
            let mut v: Vec<String> = String::from_utf8_lossy(&o.stdout).split('\0').filter(|s| !s.is_empty()).map(String::from).collect();
            v.truncate(100_000);
            return Ok(v);
        }
    }
    // Not a git repo: walk with .gitignore rules.
    let rootp = PathBuf::from(&root);
    Ok(ignore::WalkBuilder::new(&rootp)
        .hidden(false)
        .require_git(false)
        .build()
        .filter_map(|r| r.ok())
        .filter(|en| en.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|en| en.path().strip_prefix(&rootp).ok().map(|p| p.to_string_lossy().to_string()))
        .filter(|p| !p.starts_with(".git/"))
        .take(100_000)
        .collect())
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    line: u32,
    col: u32,
    len: u32,
    text: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileMatches {
    path: String,
    matches: Vec<Match>,
}

fn rg_bin() -> Option<PathBuf> {
    // Bundled next to the executable in the .app, else the user's PATH.
    let exe = std::env::current_exe().ok()?;
    let bundled = exe.parent()?.join("rg");
    if bundled.is_file() {
        return Some(bundled);
    }
    shell_env::which("rg")
}

/// Parse `rg --json` output.
pub fn parse_rg_json(text: &str, root: &str) -> Vec<FileMatches> {
    let mut out: Vec<FileMatches> = vec![];
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["type"] != "match" {
            continue;
        }
        let d = &v["data"];
        let path = d["path"]["text"].as_str().unwrap_or_default();
        let rel = path.strip_prefix(root).map(|p| p.trim_start_matches('/')).unwrap_or(path).to_string();
        let text = d["lines"]["text"].as_str().unwrap_or_default().trim_end_matches('\n').to_string();
        let line_no = d["line_number"].as_u64().unwrap_or(0) as u32;
        let subs = d["submatches"].as_array().cloned().unwrap_or_default();
        let entry = match out.last_mut() {
            Some(f) if f.path == rel => f,
            _ => {
                out.push(FileMatches { path: rel.clone(), matches: vec![] });
                out.last_mut().unwrap()
            }
        };
        for s in subs {
            let start = s["start"].as_u64().unwrap_or(0) as usize;
            let end = s["end"].as_u64().unwrap_or(0) as usize;
            // byte offsets → char columns
            let col = text.get(..start.min(text.len())).map(|p| p.chars().count()).unwrap_or(0) as u32;
            let len = text.get(start.min(text.len())..end.min(text.len())).map(|p| p.chars().count()).unwrap_or(0) as u32;
            entry.matches.push(Match { line: line_no, col: col + 1, len, text: if text.len() > 400 { text.chars().take(400).collect() } else { text.clone() } });
        }
    }
    out
}

#[tauri::command]
pub async fn search_files(root: String, query: String, regex: bool, case_sensitive: bool, glob: Option<String>) -> R<Vec<FileMatches>> {
    if query.is_empty() {
        return Ok(vec![]);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let Some(rg) = rg_bin() else { return git_grep(&root, &query, regex, case_sensitive) };
        let mut c = Command::new(rg);
        c.args(["--json", "--max-count", "200", "--max-columns", "1000", "--hidden", "-g", "!.git"]);
        if !regex {
            c.arg("--fixed-strings");
        }
        c.arg(if case_sensitive { "--case-sensitive" } else { "--smart-case" });
        if let Some(g) = glob.filter(|g| !g.trim().is_empty()) {
            c.args(["-g", g.trim()]);
        }
        c.arg("--").arg(&query).arg(&root);
        let out = c.output().map_err(e)?;
        let mut res = parse_rg_json(&String::from_utf8_lossy(&out.stdout), &root);
        res.truncate(2000);
        Ok(res)
    })
    .await
    .map_err(e)?
}

/// Fallback when ripgrep isn't installed: `git grep` (tracked + untracked files).
fn git_grep(root: &str, query: &str, regex: bool, case_sensitive: bool) -> R<Vec<FileMatches>> {
    let mut c = Command::new("git");
    c.args(["-C", root, "grep", "-n", "--column", "-I", "--untracked", "--no-color", "-m", "200"]);
    c.arg(if regex { "-E" } else { "-F" });
    if !case_sensitive {
        c.arg("-i");
    }
    c.arg("-e").arg(query);
    let out = c.envs(shell_env::login_env()).output().map_err(e)?;
    let mut res: Vec<FileMatches> = vec![];
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = line.splitn(4, ':');
        let (Some(path), Some(ln), Some(col), Some(text)) = (it.next(), it.next(), it.next(), it.next()) else { continue };
        let m = Match { line: ln.parse().unwrap_or(0), col: col.parse().unwrap_or(1), len: query.chars().count() as u32, text: text.chars().take(400).collect() };
        match res.last_mut() {
            Some(f) if f.path == path => f.matches.push(m),
            _ => res.push(FileMatches { path: path.into(), matches: vec![m] }),
        }
    }
    Ok(res)
}

/// Replace every match in the given files. Returns the number of replacements.
#[tauri::command]
pub fn replace_all(root: String, paths: Vec<String>, query: String, replacement: String, regex: bool, case_sensitive: bool) -> R<usize> {
    let pat = if regex { query.clone() } else { regex::escape(&query) };
    let re = regex::RegexBuilder::new(&pat).case_insensitive(!case_sensitive && !query.chars().any(|c| c.is_uppercase())).build().map_err(e)?;
    let mut n = 0;
    for rel in paths {
        let p = PathBuf::from(&root).join(&rel);
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let count = re.find_iter(&text).count();
        if count == 0 {
            continue;
        }
        let new = if regex { re.replace_all(&text, replacement.as_str()) } else { re.replace_all(&text, regex::NoExpand(&replacement)) };
        std::fs::write(&p, new.as_bytes()).map_err(e)?;
        n += count;
    }
    Ok(n)
}

fn has_any(root: &Path, names: &[&str]) -> bool {
    names.iter().any(|n| root.join(n).exists())
}

/// Which formatter applies to `path` in this project, if its config exists.
pub fn formatter_for(root: &Path, path: &str) -> Option<(&'static str, Vec<String>)> {
    let ext = Path::new(path).extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
    let pkg = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
    let web = ["ts", "tsx", "js", "jsx", "mjs", "cjs", "json", "css", "scss", "less", "html", "md", "yaml", "yml", "vue", "graphql"];
    if web.contains(&ext.as_str()) {
        if has_any(root, &["biome.json", "biome.jsonc"]) {
            return Some(("biome", vec!["npx".into(), "--no-install".into(), "biome".into(), "format".into(), format!("--stdin-file-path={path}")]));
        }
        if has_any(root, &[".prettierrc", ".prettierrc.json", ".prettierrc.js", ".prettierrc.cjs", ".prettierrc.mjs", ".prettierrc.yaml", ".prettierrc.yml", ".prettierrc.toml", "prettier.config.js", "prettier.config.cjs", "prettier.config.mjs"]) || pkg.contains("\"prettier\":") {
            return Some(("prettier", vec!["npx".into(), "--no-install".into(), "prettier".into(), "--stdin-filepath".into(), path.into()]));
        }
    }
    if ext == "py" {
        let py = std::fs::read_to_string(root.join("pyproject.toml")).unwrap_or_default();
        if py.contains("[tool.black]") {
            return Some(("black", vec!["black".into(), "-q".into(), "-".into()]));
        }
    }
    if ext == "go" {
        return Some(("gofmt", vec!["gofmt".into()]));
    }
    None
}

/// Format text with the project's formatter. Ok(None) when no formatter applies.
#[tauri::command]
pub async fn format_text(root: String, path: String, text: String) -> R<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let rootp = PathBuf::from(&root);
        let Some((name, argv)) = formatter_for(&rootp, &path) else { return Ok(None) };
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&rootp)
            .env_clear()
            .envs(shell_env::login_env())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|x| format!("{name}: {x}"))?;
        child.stdin.take().unwrap().write_all(text.as_bytes()).map_err(e)?;
        let out = child.wait_with_output().map_err(e)?;
        if !out.status.success() {
            return Err(format!("{name} failed: {}", String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("")));
        }
        Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
    })
    .await
    .map_err(e)?
}

/// `git show HEAD:<path>` for the diff gutter (None for new files / no repo).
#[tauri::command]
pub fn head_text(path: String) -> R<Option<String>> {
    let p = PathBuf::from(&path);
    let dir = p.parent().ok_or("bad path")?;
    let top = Command::new("git").args(["-C"]).arg(dir).args(["rev-parse", "--show-toplevel"]).output().map_err(e)?;
    if !top.status.success() {
        return Ok(None);
    }
    let top = String::from_utf8_lossy(&top.stdout).trim().to_string();
    let rel = p.strip_prefix(&top).map_err(e)?.to_string_lossy().to_string();
    let out = Command::new("git").args(["-C", &top, "show", &format!("HEAD:{rel}")]).output().map_err(e)?;
    Ok(out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Changed line ranges (1-based, inclusive, in `new`) between two versions of a file.
pub fn changed_ranges(old: &str, new: &str) -> Vec<(u32, u32)> {
    let diff = similar::TextDiff::from_lines(old, new);
    let mut out: Vec<(u32, u32)> = vec![];
    for op in diff.ops() {
        let r = op.new_range();
        match op.tag() {
            similar::DiffTag::Insert | similar::DiffTag::Replace if !r.is_empty() => {
                let (a, b) = (r.start as u32 + 1, r.end as u32);
                match out.last_mut() {
                    Some(last) if last.1 + 1 >= a => last.1 = b.max(last.1),
                    _ => out.push((a, b)),
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rg_json() {
        let t = r#"{"type":"begin","data":{"path":{"text":"/r/src/a.ts"}}}
{"type":"match","data":{"path":{"text":"/r/src/a.ts"},"lines":{"text":"const fee = calcFee(x);\n"},"line_number":12,"submatches":[{"match":{"text":"Fee"},"start":15,"end":18}]}}
{"type":"match","data":{"path":{"text":"/r/src/b.ts"},"lines":{"text":"fee\n"},"line_number":1,"submatches":[{"match":{"text":"fee"},"start":0,"end":3}]}}"#;
        let r = parse_rg_json(t, "/r");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].path, "src/a.ts");
        assert_eq!(r[0].matches[0], Match { line: 12, col: 16, len: 3, text: "const fee = calcFee(x);".into() });
    }

    #[test]
    fn ranges() {
        assert_eq!(changed_ranges("a\nb\nc\n", "a\nB\nc\nd\n"), vec![(2, 2), (4, 4)]);
        assert_eq!(changed_ranges("a\n", "x\ny\na\n"), vec![(1, 2)]);
        assert!(changed_ranges("a\nb\n", "a\n").is_empty());
    }

    #[test]
    fn formatter_detection() {
        let d = std::env::temp_dir().join(format!("fleet-fmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(formatter_for(&d, "a.ts").is_none());
        std::fs::write(d.join(".prettierrc"), "{}").unwrap();
        assert_eq!(formatter_for(&d, "a.ts").unwrap().0, "prettier");
        std::fs::write(d.join("biome.json"), "{}").unwrap();
        assert_eq!(formatter_for(&d, "a.ts").unwrap().0, "biome");
        assert_eq!(formatter_for(&d, "x.go").unwrap().0, "gofmt");
        let _ = std::fs::remove_dir_all(&d);
    }
}

// ------------------------------------------------------------------ ⌘I quick edit

pub fn quick_edit_prompt(path: &str, text: &str, start: u32, end: u32, request: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let (s, e) = ((start.max(1) - 1) as usize, (end as usize).min(lines.len()));
    // Context window around the selection keeps the prompt small for big files.
    let ctx_from = s.saturating_sub(120);
    let ctx_to = (e + 120).min(lines.len());
    let numbered: String = lines[ctx_from..ctx_to].iter().enumerate().map(|(i, l)| format!("{:>5}  {}\n", ctx_from + i + 1, l)).collect();
    let selected = lines.get(s..e).map(|v| v.join("\n")).unwrap_or_default();
    format!(
        "You are editing {path}. Change ONLY lines {start}-{end} according to the request.\n\
         Reply with the full replacement for those lines inside <replacement></replacement> tags and nothing else.\n\
         Keep the surrounding indentation style. Do not include line numbers.\n\n\
         <request>\n{request}\n</request>\n\n<selected_lines>\n{selected}\n</selected_lines>\n\n<file_context>\n{numbered}</file_context>"
    )
}

pub fn parse_replacement(answer: &str) -> Option<String> {
    let a = answer.find("<replacement>")? + "<replacement>".len();
    let b = answer.rfind("</replacement>")?;
    let mut r = answer.get(a..b)?.to_string();
    if r.starts_with('\n') {
        r.remove(0);
    }
    if r.ends_with('\n') {
        r.pop();
    }
    Some(r)
}

#[tauri::command]
pub async fn quick_edit(root: String, path: String, text: String, start: u32, end: u32, request: String, model: Option<String>) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let claude = shell_env::which("claude").ok_or("claude not found on your PATH")?;
        let prompt = quick_edit_prompt(&path, &text, start, end, &request);
        let cwd = if Path::new(&root).is_dir() { PathBuf::from(&root) } else { crate::persist::app_dir() };
        let ans = crate::titler::run_claude_p(&claude, model.as_deref().unwrap_or("sonnet"), &cwd, &prompt).map_err(e)?;
        parse_replacement(&ans).ok_or_else(|| "Claude didn't return a replacement.".to_string())
    })
    .await
    .map_err(e)?
}

#[cfg(test)]
mod quick_tests {
    use super::*;

    #[test]
    fn prompt_and_parse() {
        let p = quick_edit_prompt("a.ts", "one\ntwo\nthree\n", 2, 2, "uppercase");
        assert!(p.contains("<selected_lines>\ntwo\n</selected_lines>"));
        assert!(p.contains("    2  two"));
        assert_eq!(parse_replacement("sure\n<replacement>\nTWO\n</replacement>").as_deref(), Some("TWO"));
        assert!(parse_replacement("nope").is_none());
    }
}

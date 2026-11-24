//! fleet.yml: the project's config at the repo root, on main. Hand-edited YAML,
//! validated against schema/fleet.schema.json; errors carry line numbers.
use crate::model::ProcessDef;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::OnceLock;

pub const SCHEMA: &str = include_str!("../../schema/fleet.schema.json");
pub const FILE: &str = "fleet.yml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FleetYml {
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub processes: Vec<YmlProcess>,
    #[serde(default)]
    pub sync: SyncCfg,
    #[serde(default)]
    pub claude: ClaudeCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YmlProcess {
    pub name: String,
    pub command: String,
    #[serde(default = "dot")]
    pub cwd: String,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub auto_restart: bool,
    #[serde(default)]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "prefix")]
    pub branch_prefix: String,
    #[serde(default = "interval")]
    pub fetch_interval_sec: u64,
    #[serde(default)]
    pub setup: Option<String>,
    #[serde(default)]
    pub checks: Vec<String>,
    #[serde(default)]
    pub deploy_command: Option<String>,
    #[serde(default = "squash")]
    pub merge_style: String,
    #[serde(default)]
    pub main_is_protected: bool,
    #[serde(default = "yes")]
    pub wip_on_quit: bool,
    #[serde(default)]
    pub chat_sync: bool,
}

impl Default for SyncCfg {
    fn default() -> Self {
        SyncCfg {
            enabled: false,
            branch_prefix: prefix(),
            fetch_interval_sec: interval(),
            setup: None,
            checks: vec![],
            deploy_command: None,
            merge_style: squash(),
            main_is_protected: false,
            wip_on_quit: true,
            chat_sync: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCfg {
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub title_model: Option<String>,
}

fn dot() -> String {
    ".".into()
}
fn prefix() -> String {
    "fleet/".into()
}
fn interval() -> u64 {
    120
}
fn squash() -> String {
    "squash".into()
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YmlError {
    /// 1-based; 0 when unknown.
    pub line: usize,
    pub message: String,
}

fn validator() -> &'static jsonschema::Validator {
    static V: OnceLock<jsonschema::Validator> = OnceLock::new();
    V.get_or_init(|| {
        let schema: serde_json::Value = serde_json::from_str(SCHEMA).expect("bundled schema is JSON");
        jsonschema::validator_for(&schema).expect("bundled schema is valid")
    })
}

/// "/processes/0/name" → "processes[0].name"
fn pretty_path(pointer: &str) -> String {
    let mut out = String::new();
    for seg in pointer.split('/').filter(|s| !s.is_empty()) {
        let seg = seg.replace("~1", "/").replace("~0", "~");
        if seg.chars().all(|c| c.is_ascii_digit()) {
            out.push_str(&format!("[{seg}]"));
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(&seg);
        }
    }
    out
}

/// Line of the node at a JSON pointer, using marked-yaml's spans.
fn line_of(text: &str, pointer: &str) -> usize {
    let Ok(root) = marked_yaml::parse_yaml(0, text) else { return 0 };
    let mut node = &root;
    let mut line = node.span().start().map(|m| m.line()).unwrap_or(0);
    for seg in pointer.split('/').filter(|s| !s.is_empty()) {
        let seg = seg.replace("~1", "/").replace("~0", "~");
        let next = match node {
            marked_yaml::Node::Mapping(m) => m.get_node(&seg),
            marked_yaml::Node::Sequence(s) => seg.parse::<usize>().ok().and_then(|i| s.get_node(i)),
            _ => None,
        };
        match next {
            Some(n) => {
                node = n;
                if let Some(l) = n.span().start().map(|m| m.line()) {
                    line = l;
                }
            }
            None => break,
        }
    }
    line
}

/// Parse and validate. Ok(config) or the list of errors with line numbers.
pub fn parse(text: &str) -> Result<FleetYml, Vec<YmlError>> {
    let value: serde_json::Value = match serde_yaml_ng::from_str::<serde_json::Value>(text) {
        Ok(v) => v,
        Err(e) => {
            let line = e.location().map(|l| l.line()).unwrap_or(0);
            let msg = e.to_string();
            // serde_yaml appends " at line X column Y"; the line is shown separately.
            let msg = msg.split(" at line ").next().unwrap_or(&msg).to_string();
            return Err(vec![YmlError { line, message: msg }]);
        }
    };
    if value.is_null() {
        return Err(vec![YmlError { line: 1, message: "fleet.yml is empty".into() }]);
    }
    let mut errors: Vec<YmlError> = validator()
        .iter_errors(&value)
        .map(|e| {
            let mut ptr = e.instance_path().as_str().to_string();
            let path = pretty_path(&ptr);
            let what = e.to_string();
            // Point unknown keys at their own line, not their parent's.
            if let Some(key) = what.split('\'').nth(1).filter(|_| what.contains("was unexpected")) {
                ptr = format!("{ptr}/{}", key.replace('~', "~0").replace('/', "~1"));
            }
            YmlError { line: line_of(text, &ptr).max(1), message: if path.is_empty() { what } else { format!("{path}: {what}") } }
        })
        .collect();
    if !errors.is_empty() {
        errors.sort_by_key(|e| e.line);
        return Err(errors);
    }
    serde_json::from_value(value).map_err(|e| vec![YmlError { line: 0, message: e.to_string() }])
}

/// fleet.yml processes as fleet process definitions (ids are stable per name).
pub fn to_process_defs(cfg: &FleetYml) -> Vec<ProcessDef> {
    cfg.processes
        .iter()
        .map(|p| ProcessDef {
            id: format!("yml-{}", crate::worktree::slugify(&p.name, &|_| false)),
            name: p.name.clone(),
            command: p.command.clone(),
            cwd: p.cwd.clone(),
            env: Default::default(),
            env_from: p.env.clone(),
            auto_start: p.auto_start,
            auto_restart: p.auto_restart,
            port: p.port,
        })
        .collect()
}

fn yaml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap() // JSON strings are valid YAML scalars
}

/// Install command inferred from the lockfile, for `sync.setup`.
pub fn infer_setup(root: &Path) -> Option<String> {
    if root.join("pnpm-lock.yaml").exists() {
        Some("pnpm install --frozen-lockfile".into())
    } else if root.join("yarn.lock").exists() {
        Some("yarn install --frozen-lockfile".into())
    } else if root.join("bun.lockb").exists() || root.join("bun.lock").exists() {
        Some("bun install".into())
    } else if root.join("package-lock.json").exists() {
        Some("npm ci".into())
    } else if root.join("package.json").exists() {
        Some("npm install".into())
    } else {
        None
    }
}

/// Checks from package.json scripts (lint, typecheck, test).
pub fn infer_checks(root: &Path) -> Vec<String> {
    let Some(pkg) = std::fs::read(root.join("package.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else { return vec![] };
    let runner = crate::processes::package_runner(root);
    let scripts = pkg.get("scripts").and_then(|s| s.as_object()).cloned().unwrap_or_default();
    ["lint", "typecheck", "test"].iter().filter(|s| scripts.contains_key(**s)).map(|s| if *s == "test" && runner == "npm" { "npm test".to_string() } else { crate::processes::run_script(runner, s) }).collect()
}

/// A starter fleet.yml for a project (processes from the project or detected).
pub fn starter(project: &str, processes: &[ProcessDef], setup: Option<&str>, checks: &[String]) -> String {
    let mut y = String::new();
    y.push_str("# fleet project config. Reaches main through Ship, like code.\n");
    y.push_str("# Never put secret values here: `env` lists variable names, values come from your .env files.\n");
    y.push_str(&format!("project: {}\n", yaml_str(project)));
    if processes.is_empty() {
        y.push_str("processes: []\n");
    } else {
        y.push_str("processes:\n");
        for p in processes {
            y.push_str(&format!("  - name: {}\n    command: {}\n    cwd: {}\n", yaml_str(&p.name), yaml_str(&p.command), yaml_str(if p.cwd.is_empty() { "." } else { &p.cwd })));
            let mut names: Vec<String> = p.env_from.clone();
            names.extend(p.env.keys().cloned());
            if !names.is_empty() {
                y.push_str(&format!("    env: [{}]\n", names.join(", ")));
            }
            y.push_str(&format!("    autoStart: {}\n    autoRestart: {}\n", p.auto_start, p.auto_restart));
            if let Some(port) = p.port {
                y.push_str(&format!("    port: {port}\n"));
            }
        }
    }
    y.push_str("sync:\n  enabled: true\n  branchPrefix: fleet/\n  fetchIntervalSec: 120\n");
    if let Some(s) = setup {
        y.push_str(&format!("  setup: {}   # runs in each new session worktree\n", yaml_str(s)));
    }
    if checks.is_empty() {
        y.push_str("  checks: []\n");
    } else {
        y.push_str(&format!("  checks: [{}]\n", checks.iter().map(|c| yaml_str(c)).collect::<Vec<_>>().join(", ")));
    }
    y.push_str("  # deployCommand: ./deploy.sh   # optional; Ship runs it after main is pushed\n");
    y.push_str("  mergeStyle: squash\n  mainIsProtected: false\n");
    y.push_str("claude:\n  defaultArgs: []\n  titleModel: haiku\n");
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"project: atrium-api
processes:
  - name: dev
    command: npm run dev
    cwd: .
    env: [DATABASE_URL]   # names only
    autoStart: true
    autoRestart: true
sync:
  enabled: true
  branchPrefix: fleet/
  fetchIntervalSec: 120
  checks: [npm run lint, npm run typecheck, npm test]
  deployCommand: ./deploy.sh all
  mergeStyle: squash
  mainIsProtected: false
claude:
  defaultArgs: [--permission-mode, auto]
  titleModel: haiku
"#;

    #[test]
    fn spec_example_is_valid() {
        let c = parse(GOOD).unwrap();
        assert_eq!(c.project, "atrium-api");
        assert!(c.sync.enabled);
        assert_eq!(c.sync.checks.len(), 3);
        assert_eq!(c.processes[0].env, vec!["DATABASE_URL"]);
        assert_eq!(c.claude.default_args, vec!["--permission-mode", "auto"]);
        assert!(c.sync.wip_on_quit);
        let defs = to_process_defs(&c);
        assert_eq!(defs[0].id, "yml-dev");
        assert_eq!(defs[0].env_from, vec!["DATABASE_URL"]);
    }

    #[test]
    fn missing_field_has_line() {
        let t = "project: x\nprocesses:\n  - name: dev\n    cwd: .\n";
        let e = parse(t).unwrap_err();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].line, 3, "{e:?}");
        assert!(e[0].message.contains("processes[0]") && e[0].message.contains("command"), "{e:?}");
    }

    #[test]
    fn wrong_type_has_line() {
        let t = "project: x\nsync:\n  enabled: true\n  fetchIntervalSec: soon\n";
        let e = parse(t).unwrap_err();
        assert_eq!(e[0].line, 4, "{e:?}");
        assert!(e[0].message.starts_with("sync.fetchIntervalSec"), "{e:?}");
    }

    #[test]
    fn unknown_key_and_secret_shaped_env() {
        let t = "project: x\nsync:\n  enabeld: true\nprocesses:\n  - name: a\n    command: b\n    env: [\"DATABASE_URL=postgres://u:p@h\"]\n";
        let e = parse(t).unwrap_err();
        assert!(e.iter().any(|x| x.line == 3 && x.message.contains("enabeld")), "{e:?}");
        assert!(e.iter().any(|x| x.line == 7 && x.message.contains("env[0]")), "{e:?}");
    }

    #[test]
    fn bad_yaml_has_line() {
        let t = "project: x\nprocesses:\n  - name: a\n   command: [unclosed\n";
        let e = parse(t).unwrap_err();
        assert_eq!(e.len(), 1);
        assert!(e[0].line >= 3 && e[0].line <= 5, "{e:?}");
        assert!(parse("").is_err());
    }

    #[test]
    fn starter_is_valid() {
        let defs = vec![ProcessDef { id: "a".into(), name: "dev".into(), command: "npm run dev".into(), cwd: ".".into(), auto_start: true, ..Default::default() }];
        let y = starter("proj \"x\"", &defs, Some("npm ci"), &["npm run lint".into(), "npm test".into()]);
        let c = parse(&y).unwrap_or_else(|e| panic!("{e:?}\n{y}"));
        assert_eq!(c.project, "proj \"x\"");
        assert_eq!(c.sync.setup.as_deref(), Some("npm ci"));
        assert_eq!(c.processes.len(), 1);
        assert!(parse(&starter("p", &[], None, &[])).is_ok());
    }
}

//! Core data model. Mirrors src/types.ts on the frontend (camelCase over the wire).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    NeedsPermission,
    NeedsReply,
    Working,
    Done,
    Failed,
    Idle,
    Queued,
}

impl SessionState {
    pub fn needs_you(self) -> bool {
        matches!(self, SessionState::NeedsPermission | SessionState::NeedsReply)
    }
    pub fn symbol(self) -> &'static str {
        match self {
            SessionState::NeedsPermission | SessionState::NeedsReply => "●",
            SessionState::Working => "◐",
            SessionState::Done => "✓",
            SessionState::Failed => "✕",
            SessionState::Idle => "○",
            SessionState::Queued => "◌",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProcessDef {
    pub id: String,
    pub name: String,
    pub command: String,
    pub cwd: String,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Variable names (fleet.yml `env`) whose values come from the project's .env files.
    #[serde(default)]
    pub env_from: Vec<String>,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub auto_restart: bool,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub color_family: String,
    #[serde(default)]
    pub processes: Vec<ProcessDef>,
    #[serde(default)]
    pub default_claude_args: Vec<String>,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub last_activity: i64,
    /// Detected scripts were offered (or dismissed) once.
    #[serde(default)]
    pub processes_offered: bool,
    #[serde(default)]
    pub ssh_connections: Vec<SshConnection>,
    /// GitHub sync paused on this Mac (nothing committed or pushed automatically).
    #[serde(default)]
    pub sync_paused: bool,
}

/// A server this project's sessions can work on over SSH.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SshConnection {
    pub id: String,
    pub name: String,
    /// ssh-config alias or user@host.
    pub host: String,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    /// Folder on the server the code lives in (may start with ~/).
    pub remote_path: String,
    #[serde(default)]
    pub extra_args: Option<String>,
    #[serde(default)]
    pub auto_connect: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub claude_session_id: String,
    pub glyph: String,
    pub color: String,
    pub title: String,
    #[serde(default)]
    pub title_history: Vec<String>,
    #[serde(default)]
    pub recap: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub branch: String,
    pub worktree_path: Option<String>,
    pub state: SessionState,
    /// Unix millis.
    pub state_since: i64,
    pub pending_question: Option<String>,
    /// 0..=100, None until known.
    pub context_pct: Option<f32>,
    #[serde(default)]
    pub files_touched: Vec<String>,
    pub created_at: i64,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub pinned_color: bool,
    #[serde(default)]
    pub pinned_glyph: bool,
    #[serde(default)]
    pub title_locked: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub permission_mode: Option<String>,
    // Runtime-only fields.
    #[serde(default)]
    pub alive: bool,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub last_prompt: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Lines this session changed per file (1-based inclusive ranges), for the editor gutter.
    #[serde(default)]
    pub line_marks: std::collections::HashMap<String, Vec<(u32, u32)>>,
    /// SSH connection id when this session works on a server.
    #[serde(default)]
    pub remote: Option<String>,
    /// Finished prompts (Stop events), for Fleet-Prompt trailers and the state file.
    #[serde(default)]
    pub prompt_count: u32,
    /// GitHub sync status for synced projects.
    #[serde(default)]
    pub sync: Option<SyncInfo>,
    /// Token spend and turn timing.
    #[serde(default)]
    pub metrics: SessionMetrics,
}

/// Tokens and estimated API cost (USD) for one side of a session (main thread or sub-agents).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Tally {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    /// Tokens from models missing from the price table (not in `cost`).
    pub unpriced: u64,
}

impl Tally {
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// Where the usage scanner stopped in one transcript file.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Cursor {
    pub offset: u64,
    /// Recent request ids (claude writes one line per content block, each repeating the usage).
    pub recent: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionMetrics {
    pub main: Tally,
    /// Sub-agent (Task/Agent tool) usage.
    pub sub: Tally,
    /// Finished turns (prompt → Stop).
    pub turns: u32,
    pub turn_ms: i64,
    /// Part of `turn_ms` spent waiting on the user (permission prompts, questions).
    pub wait_ms: i64,
    pub last_turn_ms: Option<i64>,
    pub last_wait_ms: Option<i64>,
    /// Start of the turn in progress.
    pub turn_started: Option<i64>,
    pub turn_wait: i64,
    /// Transcript path → scan position.
    pub cursors: std::collections::BTreeMap<String, Cursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncInfo {
    /// "pushed" | "waiting" | "conflict" | "paused" | "syncing" | "taken_over" | "error" | "new"
    pub badge: String,
    #[serde(default)]
    pub waiting: u32,
    #[serde(default)]
    pub last_push_at: Option<i64>,
    /// Files never auto-committed (.env*, keys, > 5 MB) present in the last commit attempt.
    #[serde(default)]
    pub excluded: Vec<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub last_commit: Option<String>,
    /// updatedAt of this session's file on GitHub that we've applied.
    #[serde(default)]
    pub remote_updated_at: Option<String>,
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

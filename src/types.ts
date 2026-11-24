// Mirrors src-tauri/src/model.rs
export type SessionState =
  | "needs_permission"
  | "needs_reply"
  | "working"
  | "done"
  | "failed"
  | "idle"
  | "queued";

export interface ProcessDef {
  id: string;
  name: string;
  command: string;
  cwd: string;
  env: Record<string, string>;
  /** fleet.yml `env`: names whose values come from the project's .env files. */
  envFrom?: string[];
  autoStart: boolean;
  autoRestart: boolean;
  port?: number | null;
}

export interface Project {
  id: string;
  name: string;
  rootPath: string;
  colorFamily: string;
  processes: ProcessDef[];
  defaultClaudeArgs: string[];
  collapsed: boolean;
  lastActivity: number;
  processesOffered: boolean;
  sshConnections: SshConnection[];
  /** GitHub sync paused on this Mac. */
  syncPaused?: boolean;
}

export interface SshConnection {
  id: string;
  name: string;
  /** ssh-config alias or user@host */
  host: string;
  port?: number | null;
  identityFile?: string | null;
  remotePath: string;
  extraArgs?: string | null;
  autoConnect: boolean;
}

export interface ConnStatus {
  connId: string;
  state: "connecting" | "connected" | "disconnected" | "failed";
  error?: string | null;
  since: number;
  /** Other projects' connections sharing this server connection. */
  sharedWith: number;
}

export interface Session {
  id: string;
  projectId: string;
  claudeSessionId: string;
  glyph: string;
  color: string;
  title: string;
  titleHistory: string[];
  recap: string[];
  cwd: string;
  branch: string;
  worktreePath?: string | null;
  state: SessionState;
  stateSince: number;
  pendingQuestion?: string | null;
  contextPct?: number | null;
  filesTouched: string[];
  createdAt: number;
  pinned: boolean;
  pinnedColor: boolean;
  pinnedGlyph: boolean;
  titleLocked: boolean;
  archived: boolean;
  permissionMode?: string | null;
  alive: boolean;
  transcriptPath?: string | null;
  lastPrompt?: string | null;
  model?: string | null;
  exitCode?: number | null;
  /** Lines this session changed per absolute path (1-based inclusive ranges). */
  lineMarks?: Record<string, [number, number][]>;
  /** SSH connection id when the session works on a server. */
  remote?: string | null;
  promptCount?: number;
  sync?: SyncInfo | null;
  /** Token spend and turn timing (absent on sessions from older builds until their first reply). */
  metrics?: SessionMetrics;
}

/** Tokens and estimated API-list-price cost (USD). */
export interface Tally {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  cost: number;
  /** Tokens from models fleet has no price for (not in `cost`). */
  unpriced: number;
}

export interface SessionMetrics {
  main: Tally;
  /** Sub-agent usage. */
  sub: Tally;
  turns: number;
  turnMs: number;
  waitMs: number;
  lastTurnMs?: number | null;
  lastWaitMs?: number | null;
  /** Start (unix ms) of the turn in progress. */
  turnStarted?: number | null;
  turnWait: number;
}

/** usage_report row: one project on one local day. */
export interface UsageDay {
  day: string;
  project: string;
  cost: number;
  subCost: number;
  tokens: number;
  subTokens: number;
  turns: number;
  turnMs: number;
  waitMs: number;
  maxTurnMs: number;
}

export type SyncBadge = "pushed" | "waiting" | "conflict" | "paused" | "syncing" | "taken_over" | "error" | "new";

export interface SyncInfo {
  badge: SyncBadge;
  waiting: number;
  lastPushAt?: number | null;
  /** Files never auto-committed (.env*, keys, > 5 MB) seen in the last commit. */
  excluded: string[];
  detail?: string | null;
  lastCommit?: string | null;
  remoteUpdatedAt?: string | null;
}

export interface OtherSession {
  projectId: string; branch: string; glyph: string; color: string; title: string; recap: string[];
  state: string; prompts: number; lastMachine: string; updatedAt: string; commits: number;
  activeOn?: string | null; activeAgo?: string | null; needsRebase: boolean;
}
export interface ShipCandidate {
  branch: string; title: string; glyph: string; color: string; commits: number; stat: string; machine: string;
  state: string; ready: boolean; reason?: string | null; sessionId?: string | null;
}
export interface ShipInfo {
  candidates: ShipCandidate[]; checks: string[]; deployCommand?: string | null; mergeStyle: string;
  protected: boolean; defaultBranch: string; deploys: DeployEntry[]; shipping: boolean;
}
export type BranchResult =
  | { kind: "merged"; commit: string }
  | { kind: "conflict"; files: string[]; output: string }
  | { kind: "checksFailed"; check: string; output: string }
  | { kind: "empty" };
export interface ShipOutcome {
  results: [string, BranchResult][]; mainCommit?: string | null; pushed: boolean; tag?: string | null;
  deployCode?: number | null; deployOutput?: string | null; error?: string | null; others: string[];
}
export interface ShippedCommit { commit: string; subject: string; at: number; branch: string }
export interface WorkflowFinding { path: string; before: string; after: string }
export interface GhRepo { nameWithOwner: string; description?: string | null; isPrivate: boolean; updatedAt: string }

// ---- GitHub sync (mirrors src-tauri/src/sync)
export interface YmlError { line: number; message: string }
export interface FleetYml {
  project: string;
  processes: { name: string; command: string; cwd: string; env: string[]; autoStart: boolean; autoRestart: boolean; port?: number | null }[];
  sync: { enabled: boolean; branchPrefix: string; fetchIntervalSec: number; setup?: string | null; checks: string[]; deployCommand?: string | null; mergeStyle: string; mainIsProtected: boolean; wipOnQuit: boolean; chatSync: boolean };
  claude: { defaultArgs: string[]; titleModel?: string | null };
}
export interface DeployEntry { tag: string; commit: string; branches: string[]; machine: string; at: string; result: string }
export interface ProjectCfg {
  projectId: string;
  config?: FleetYml | null;
  errors: YmlError[];
  source: "main" | "local" | "checkout" | "none";
  undeployed: boolean;
  hasOrigin: boolean;
  originUrl: string;
  defaultBranch: string;
  enabled: boolean;
  paused: boolean;
  lastDeploy?: DeployEntry | null;
  localChanges: number;
  localAhead: number;
}
export interface SyncStatus { lastSyncAt?: number | null; offline: boolean; waiting: number; pausedAll: boolean; anySynced: boolean }
export interface GhStatus { installed: boolean; authed: boolean; user?: string | null; message?: string | null }
export interface SyncCheck {
  isRepo: boolean; hasOrigin: boolean; originUrl: string; isGithub: boolean; gh: GhStatus;
  mainHasFleetYml: boolean; defaultBranch: string; enabled: boolean; dirtyMain: boolean;
}

export interface Snapshot {
  projects: Project[];
  sessions: Session[];
  ui: Partial<UiPrefs> | null;
  restored: number;
  warnings: string[];
  runs: ProcessRun[];
}

export interface UiPrefs {
  leftW: number;
  rightW: number;
  leftHidden: boolean;
  rightHidden: boolean;
  activeSessionId: string | null;
}

export type RunStatus = "running" | "exited" | "crashed" | "stopped";

export interface ProcessRun {
  key: string;
  projectId: string;
  defId: string;
  pid?: number | null;
  status: RunStatus;
  exitCode?: number | null;
  detectedPorts: number[];
  startedAt: number;
  restarts: number;
  nextRestartAt?: number | null;
  stopping: boolean;
  detached: boolean;
  /** Runs in this session's worktree. */
  sessionId?: string | null;
  baseLabel?: string | null;
}

export const procKey = (projectId: string, defId: string) => `proc:${projectId}:${defId}`;

// ---- git (mirrors src-tauri/src/git.rs)
export interface FileEntry {
  path: string;
  origPath?: string | null;
  index: string;
  worktree: string;
  conflicted: boolean;
  untracked: boolean;
  added?: number | null;
  removed?: number | null;
  stagedAdded?: number | null;
  stagedRemoved?: number | null;
}
export interface GitStatus {
  branch: string;
  oid: string;
  upstream?: string | null;
  ahead: number;
  behind: number;
  files: FileEntry[];
}
export interface RepoStatus {
  isRepo: boolean;
  status?: GitStatus | null;
  top?: string | null;
  merging: boolean;
}
export interface Hunk {
  header: string;
  oldStart: number;
  oldLen: number;
  newStart: number;
  newLen: number;
  lines: string[];
}
export interface DiffResult {
  original: string;
  modified: string;
  diff: { header: string; hunks: Hunk[] };
  binary: boolean;
}
export interface Commit {
  hash: string;
  parents: string[];
  author: string;
  time: number;
  subject: string;
  refs: string;
}
export interface Branch {
  name: string;
  full: string;
  remote: boolean;
  upstream?: string | null;
  ahead: number;
  behind: number;
  gone: boolean;
  time: number;
  head: boolean;
  worktree?: string | null;
}
export interface Worktree {
  path: string;
  head: string;
  branch?: string | null;
  bare: boolean;
  detached: boolean;
}
export interface Stash {
  name: string;
  message: string;
  time: number;
}
export interface GitError {
  message: string;
  fix?: string | null;
  fixLabel?: string | null;
  raw: string;
}
export interface OutputEntry {
  id: number;
  root: string;
  cmd: string;
  output: string;
  code: number;
  ms: number;
  at: number;
  /** Run by GitHub sync. */
  auto?: boolean;
}

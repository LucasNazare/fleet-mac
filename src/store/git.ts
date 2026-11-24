import { create } from "zustand";
import { backend, invoke } from "../ipc";
import type { Branch, Commit, GitError, OutputEntry, RepoStatus, Stash, Worktree } from "../types";
import { useFleet } from "./store";
import { needsYou } from "../theme";

export interface Repo {
  root: string;
  status: RepoStatus | null;
  log: Commit[] | null;
  branches: Branch[] | null;
  worktrees: Worktree[] | null;
  stashes: Stash[] | null;
  busy: string | null;
  error: GitError | null;
}

interface GitStore {
  repos: Record<string, Repo>;
  output: OutputEntry[];
  outputOpen: boolean;
  /** Output drawer shows only GitHub sync's automatic commands (all checkouts). */
  autoOnly: boolean;
  tab: "changes" | "history" | "branches" | "stash";
  onlySessionFiles: boolean;
  init(): Promise<void>;
  ensure(root: string): void;
  refresh(root: string, what?: ("status" | "log" | "branches" | "stashes")[]): Promise<void>;
  /** Run a git action with busy/error handling; refreshes afterwards. */
  act(root: string, label: string, fn: () => Promise<unknown>): Promise<boolean>;
  set(p: Partial<GitStore>): void;
}

const blank = (root: string): Repo => ({ root, status: null, log: null, branches: null, worktrees: null, stashes: null, busy: null, error: null });

const toErr = (e: unknown): GitError =>
  typeof e === "object" && e && "message" in e ? (e as GitError) : { message: String(e), raw: String(e) };

const lastRefresh: Record<string, number> = {};
let gitInitDone = false;

export const useGit = create<GitStore>((set, get) => ({
  repos: {},
  output: [],
  outputOpen: false,
  autoOnly: false,
  tab: "changes",
  onlySessionFiles: false,

  async init() {
    // React StrictMode (dev) runs effects twice; listeners must be registered once.
    if (gitInitDone) return;
    gitInitDone = true;
    const be = await backend();
    await be.listen<OutputEntry>("git-output", (e) => set((s) => ({ output: [...s.output.slice(-299), e] })));
    await be.listen<string>("git-changed", (root) => {
      // Never refresh faster than once per second per repo.
      const now = Date.now();
      if (now - (lastRefresh[root] ?? 0) < 1000) {
        window.setTimeout(() => void get().refresh(root), 1000 - (now - (lastRefresh[root] ?? 0)));
        return;
      }
      void get().refresh(root);
    });
  },

  ensure(root) {
    if (!get().repos[root]) {
      set((s) => ({ repos: { ...s.repos, [root]: blank(root) } }));
      void invoke("git_watch", { root }).catch(() => {});
      void get().refresh(root, ["status", "branches"]);
    }
  },

  async refresh(root, what = ["status"]) {
    lastRefresh[root] = Date.now();
    const patch: Partial<Repo> = {};
    await Promise.all(
      what.map(async (w) => {
        try {
          if (w === "status") patch.status = await invoke<RepoStatus>("git_status", { root });
          if (w === "log") patch.log = await invoke<Commit[]>("git_log", { root, limit: 200 });
          if (w === "branches") {
            const b = await invoke<{ branches: Branch[]; worktrees: Worktree[] }>("git_branches", { root });
            patch.branches = b.branches;
            patch.worktrees = b.worktrees;
          }
          if (w === "stashes") patch.stashes = await invoke<Stash[]>("git_stashes", { root });
        } catch {
          /* not a repo yet, etc. */
        }
      }),
    );
    set((s) => ({ repos: { ...s.repos, [root]: { ...(s.repos[root] ?? blank(root)), ...patch } } }));
  },

  async act(root, label, fn) {
    const upd = (p: Partial<Repo>) => set((s) => ({ repos: { ...s.repos, [root]: { ...(s.repos[root] ?? blank(root)), ...p } } }));
    upd({ busy: label, error: null });
    try {
      await fn();
      upd({ busy: null });
      const tab = get().tab;
      await get().refresh(root, ["status", "branches", ...(tab === "history" ? (["log"] as const) : []), ...(tab === "stash" ? (["stashes"] as const) : [])]);
      return true;
    } catch (e) {
      upd({ busy: null, error: toErr(e) });
      await get().refresh(root, ["status"]);
      return false;
    }
  },

  set(p) {
    set(p);
  },
}));

/** The checkout the git panel shows: the selected session's worktree or its project root. */
export function currentRoot(): { root: string; projectId: string } | null {
  const st = useFleet.getState();
  const s = st.activeSessionId ? st.sessions[st.activeSessionId] : null;
  const pid = s?.projectId ?? st.editorProjectId ?? st.projects[0]?.id;
  const p = st.projects.find((x) => x.id === pid);
  if (!p) return null;
  return { root: s?.worktreePath || p.rootPath, projectId: p.id };
}

/** Sessions working in a checkout (warn before checkout / pull / rebase). */
export function workingSessionsIn(root: string) {
  const st = useFleet.getState();
  return Object.values(st.sessions).filter((s) => !s.archived && s.alive && (s.worktreePath || st.projects.find((p) => p.id === s.projectId)?.rootPath) === root && (s.state === "working" || needsYou(s.state)));
}

/** Sessions (live) whose filesTouched include `abs`. */
export function sessionsTouching(abs: string) {
  return Object.values(useFleet.getState().sessions).filter((s) => !s.archived && s.filesTouched.includes(abs));
}

export function joinPath(root: string, rel: string) {
  return root.endsWith("/") ? root + rel : `${root}/${rel}`;
}

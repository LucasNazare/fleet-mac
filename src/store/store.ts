import { create } from "zustand";
import type { ConnStatus, ProcessDef, ProcessRun, Project, Session, Snapshot, SshConnection, UiPrefs } from "../types";
import { STATE } from "../theme";
import { backend, invoke } from "../ipc";
import * as pool from "../terminal/termPool";
import { useEditor, type EditorTab } from "./editor";

export type CenterTab = "session" | "editor" | "process";
export type Filter = null | "needs" | "working" | "done" | "idle" | "failed";

interface FleetState {
  ready: boolean;
  projects: Project[];
  sessions: Record<string, Session>;
  activeSessionId: string | null;
  /** Second pane when split (⌘\). */
  splitSessionId: string | null;
  focusedPane: 0 | 1;
  centerTab: CenterTab;
  leftW: number;
  rightW: number;
  leftHidden: boolean;
  rightHidden: boolean;
  filter: Filter;
  search: string;
  sheetOpen: boolean;
  sheetProjectId: string | null;
  toast: { text: string; kind: "info" | "error" } | null;
  warnings: string[];
  confirm: { session: Session } | null;
  runs: Record<string, ProcessRun>;
  /** Process shown in the center pane's Process tab (its key). */
  selectedProcess: string | null;
  procForm: { projectId: string; def?: ProcessDef } | null;
  sendPicker: { text: string; header: string; projectId: string | null } | null;
  quitPrompt: number | null;
  /** Detected-but-not-added processes per project. */
  suggestions: Record<string, ProcessDef[]>;
  /** Project the editor is showing (follows the selected session unless pinned by the user). */
  editorProjectId: string | null;
  sidebarMode: "sessions" | "files" | "search";
  paletteOpen: boolean;
  settingsOpen: boolean;
  /** SSH status per project connection. */
  ssh: Record<string, ConnStatus>;
  sshForm: { projectId: string; conn?: SshConnection } | null;
  /** ⌃N consoles per session. */
  consoles: Record<string, { open: boolean; height: number; tabs: string[]; active: string | null }>;
  /** Standalone remote terminals (and SSH login terminals) by id → title. */
  terminals: Record<string, string>;

  init(): Promise<void>;
  select(id: string | null): void;
  setUi(p: Partial<FleetState>): void;
  showToast(text: string, kind?: "info" | "error"): void;
  addProject(): Promise<void>;
  openProcess(key: string): void;
  /** ⌘N: start a session right away (no dialog) in `projectId` or the current project. */
  quickSession(projectId?: string, opts?: { worktree?: boolean }): Promise<void>;
  loadSuggestions(projectId: string): Promise<void>;
  newSession(projectId: string, prompt: string, permissionMode: string, size: { cols: number; rows: number }, worktree?: boolean, remote?: string | null): Promise<void>;
}

let saveTimer: number | undefined;
let fleetInitDone = false;
function persistUi(s: FleetState) {
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    const ui: UiPrefs = {
      leftW: s.leftW, rightW: s.rightW, leftHidden: s.leftHidden, rightHidden: s.rightHidden,
      activeSessionId: s.activeSessionId,
    };
    invoke("save_ui", { ui }).catch(() => {});
  }, 800);
}

export const useFleet = create<FleetState>((set, get) => ({
  ready: false,
  projects: [],
  sessions: {},
  activeSessionId: null,
  splitSessionId: null,
  focusedPane: 0,
  centerTab: "session",
  leftW: 300,
  rightW: 330,
  leftHidden: false,
  rightHidden: false,
  filter: null,
  search: "",
  sheetOpen: false,
  sheetProjectId: null,
  toast: null,
  warnings: [],
  confirm: null,
  runs: {},
  selectedProcess: null,
  procForm: null,
  sendPicker: null,
  quitPrompt: null,
  suggestions: {},
  editorProjectId: null,
  sidebarMode: "sessions",
  paletteOpen: false,
  settingsOpen: false,
  ssh: {},
  sshForm: null,
  consoles: {},
  terminals: {},

  async init() {
    // React StrictMode (dev) runs effects twice; listeners must be registered once.
    if (fleetInitDone) return;
    fleetInitDone = true;
    const be = await backend();
    await be.listen<Session>("session-updated", (s) =>
      set((st) => ({ sessions: { ...st.sessions, [s.id]: s } })),
    );
    await be.listen<string>("session-removed", (id) => {
      pool.dispose(id);
      set((st) => {
        const sessions = { ...st.sessions };
        delete sessions[id];
        return {
          sessions,
          activeSessionId: st.activeSessionId === id ? null : st.activeSessionId,
          splitSessionId: st.splitSessionId === id ? null : st.splitSessionId,
        };
      });
    });
    await be.listen<Project[]>("projects-changed", (projects) => set({ projects }));
    await be.listen<string>("focus-session", (id) => {
      if (id.startsWith("proc:")) return get().openProcess(id);
      const s = get().sessions[id];
      if (s && !s.archived) get().select(id);
    });
    await be.listen<ProcessRun>("process-updated", (r) => set((st) => ({ runs: { ...st.runs, [r.key]: r } })));
    await be.listen<string>("process-removed", (key) =>
      set((st) => {
        const runs = { ...st.runs };
        delete runs[key];
        pool.dispose(key);
        return { runs, selectedProcess: st.selectedProcess === key ? null : st.selectedProcess };
      }),
    );
    await be.listen<number>("confirm-quit", (n) => set({ quitPrompt: n }));
    await be.listen<ConnStatus>("ssh-status", (s) => set((st) => ({ ssh: { ...st.ssh, [s.connId]: s } })));
    await be.listen<{ id: string; code: number | null }>("terminal-exit", ({ id }) => {
      // A console tab whose shell ended closes itself.
      const m = /^console:(.+):\d+$/.exec(id);
      if (!m) return;
      set((st) => {
        const c = st.consoles[m[1]];
        if (!c) return st;
        const tabs = c.tabs.filter((t) => t !== id);
        pool.dispose(id);
        return { consoles: { ...st.consoles, [m[1]]: { ...c, tabs, open: c.open && tabs.length > 0, active: c.active === id ? tabs[tabs.length - 1] ?? null : c.active } } };
      });
    });
    void invoke<ConnStatus[]>("ssh_statuses").then((list) => set({ ssh: Object.fromEntries(list.map((s) => [s.connId, s])) })).catch(() => {});
    await be.listen<string>("toast", (t) => get().showToast(t, "error"));

    const snap = await invoke<Snapshot>("get_snapshot");
    const sessions = Object.fromEntries(snap.sessions.map((s) => [s.id, s]));
    const ui = snap.ui ?? {};
    const active = ui.activeSessionId && sessions[ui.activeSessionId] ? ui.activeSessionId : null;
    set({
      ready: true,
      projects: snap.projects,
      sessions,
      leftW: ui.leftW ?? 300,
      rightW: ui.rightW ?? 330,
      leftHidden: ui.leftHidden ?? false,
      rightHidden: ui.rightHidden ?? false,
      activeSessionId: active ?? orderedSessions(snap.projects, sessions)[0]?.id ?? null,
      warnings: snap.warnings,
      runs: Object.fromEntries((snap.runs ?? []).map((r) => [r.key, r])),
    });
    for (const p of snap.projects) if (!p.processesOffered) void get().loadSuggestions(p.id);
    // Reopen editor tabs from the last run.
    const savedTabs = (ui as { editorTabs?: EditorTab[]; editorActive?: Record<string, string | null> }).editorTabs;
    if (Array.isArray(savedTabs)) {
      const ok = savedTabs.filter((t) => t.kind === "file" && snap.projects.some((p) => p.id === t.projectId));
      useEditor.setState({ tabs: ok, active: (ui as { editorActive?: Record<string, string | null> }).editorActive ?? {} });
    }
    if (snap.restored > 0) get().showToast(`Restored ${snap.restored} session${snap.restored === 1 ? "" : "s"}`);
  },

  select(id) {
    const st = get();
    if (st.splitSessionId && st.focusedPane === 1) {
      if (id === st.activeSessionId) set({ focusedPane: 0 });
      else set({ splitSessionId: id });
    } else {
      if (id && id === st.splitSessionId) {
        set({ focusedPane: 1 });
      } else set({ activeSessionId: id, centerTab: st.splitSessionId === "__editor__" ? st.centerTab : "session", sidebarMode: st.sidebarMode === "files" && st.centerTab === "editor" ? st.sidebarMode : "sessions" });
    }
    // A synced session catches up with GitHub when opened (fast-forward, presence check).
    const sel = id ? get().sessions[id] : null;
    if (sel?.sync && sel.branch) void invoke("sync_open_session", { sessionId: id }).catch(() => {});
    // Files, Search and the editor follow the selected session's project.
    const pid = id ? get().sessions[id]?.projectId : null;
    if (pid && pid !== get().editorProjectId) set({ editorProjectId: pid });
    persistUi(get());
  },

  setUi(p) {
    // Opening the editor shows the file tree; going back to sessions shows the list.
    if (p.sidebarMode === undefined) {
      if (p.centerTab === "editor" && get().centerTab !== "editor" && get().sidebarMode === "sessions") p = { ...p, sidebarMode: "files" };
      if (p.centerTab === "session" && get().centerTab === "editor" && get().sidebarMode !== "sessions") p = { ...p, sidebarMode: "sessions" };
    }
    set(p as Partial<FleetState>);
    persistUi(get());
  },

  showToast(text, kind = "info") {
    set({ toast: { text, kind } });
    window.setTimeout(() => {
      if (get().toast?.text === text) set({ toast: null });
    }, kind === "error" ? 6000 : 3000);
  },

  async addProject() {
    const be = await backend();
    const path = await be.pickFolder();
    if (!path) return;
    try {
      const p = await invoke<Project>("add_project", { path });
      set((st) => ({ projects: st.projects.some((x) => x.id === p.id) ? st.projects : [...st.projects, p], editorProjectId: p.id }));
      if (!p.processesOffered) void get().loadSuggestions(p.id);
    } catch (e) {
      get().showToast(String(e), "error");
    }
  },

  async quickSession(projectId, opts) {
    const st = get();
    const cur = st.activeSessionId ? st.sessions[st.activeSessionId]?.projectId : null;
    const pid = projectId ?? cur ?? st.editorProjectId ?? orderedProjects(st.projects)[0]?.id;
    if (!pid) {
      st.showToast("Add a project first.");
      return void st.addProject();
    }
    const { useSettings } = await import("./settings");
    const mode = useSettings.getState().settings.claude.permissionMode || "default";
    await get().newSession(pid, "", mode, pool.currentSize(), opts?.worktree ?? false);
  },

  openProcess(key) {
    set({ selectedProcess: key, centerTab: "process" });
  },

  async loadSuggestions(projectId) {
    const list = await invoke<ProcessDef[]>("detect_processes", { projectId }).catch(() => []);
    set((st) => ({ suggestions: { ...st.suggestions, [projectId]: list } }));
  },

  async newSession(projectId, prompt, permissionMode, size, worktree = false, remote = null) {
    try {
      const s = await invoke<Session>("new_session", {
        projectId,
        prompt: prompt.trim() || null,
        permissionMode: permissionMode === "default" ? null : permissionMode,
        worktree,
        remote,
        cols: size.cols,
        rows: size.rows,
      });
      set((st) => ({ sessions: { ...st.sessions, [s.id]: s }, sheetOpen: false }));
      get().select(s.id);
    } catch (e) {
      get().showToast(String(e), "error");
    }
  },
}));

/** Projects ordered by most recent activity. */
export function orderedProjects(projects: Project[]): Project[] {
  return [...projects].sort((a, b) => b.lastActivity - a.lastActivity);
}

export function projectSessions(projectId: string, sessions: Record<string, Session>, archived = false): Session[] {
  return Object.values(sessions)
    .filter((s) => s.projectId === projectId && s.archived === archived)
    .sort((a, b) => a.createdAt - b.createdAt);
}

/** Live sessions in sidebar order — used for ⌘] / ⌘[. */
export function orderedSessions(projects: Project[], sessions: Record<string, Session>): Session[] {
  return orderedProjects(projects).flatMap((p) => projectSessions(p.id, sessions));
}

export function matchesFilter(s: Session, filter: Filter, search: string): boolean {
  if (filter && STATE[s.state].group !== filter) return false;
  if (search) {
    const q = search.toLowerCase();
    return s.title.toLowerCase().includes(q) || s.glyph.toLowerCase().includes(q) || (s.pendingQuestion ?? "").toLowerCase().includes(q);
  }
  return true;
}

import { create } from "zustand";
import type { OtherSession, ProjectCfg, ShipOutcome, SyncStatus } from "../types";
import { backend, invoke } from "../ipc";
import { useFleet } from "./store";

export interface ShipRun {
  running: boolean;
  log: string;
  steps: { step: string; branch?: string | null; ok?: boolean | null }[];
  outcome?: ShipOutcome | null;
}

interface SyncStore {
  configs: Record<string, ProjectCfg>;
  status: SyncStatus;
  /** Sessions on GitHub with no session on this Mac. */
  others: OtherSession[];
  /** fleet.yml editor: project, text, line to show. */
  editor: { projectId: string; text: string; line?: number } | null;
  /** GitHub sync setup dialog for a project. */
  setup: string | null;
  /** Ship sheet for a project. */
  ship: string | null;
  runs: Record<string, ShipRun>;
  reposOpen: boolean;
  init(): Promise<void>;
  openEditor(projectId: string, line?: number): Promise<void>;
}

let started = false;
const emptyRun = (): ShipRun => ({ running: false, log: "", steps: [] });

export const useSync = create<SyncStore>((set, get) => ({
  configs: {},
  status: { offline: false, waiting: 0, pausedAll: false, anySynced: false },
  others: [],
  editor: null,
  setup: null,
  ship: null,
  runs: {},
  reposOpen: false,
  async init() {
    if (started) return;
    started = true;
    const be = await backend();
    const put = (list: ProjectCfg[]) => set((s) => ({ configs: { ...s.configs, ...Object.fromEntries(list.map((c) => [c.projectId, c])) } }));
    await be.listen<ProjectCfg[]>("sync-configs", put);
    await be.listen<SyncStatus>("sync-status", (status) => set({ status }));
    await be.listen<OtherSession[]>("sync-others", (others) => set({ others }));
    await be.listen<{ sessionId: string; files: string[] }>("sync-excluded", ({ files }) =>
      useFleet.getState().showToast(`Not committed (never auto-committed): ${files.join(", ")}`, "error"),
    );
    // A session moved into its own synced space: its claude was restarted there.
    await be.listen<string>("session-respawned", (id) => {
      void import("../terminal/termPool").then(async (pool) => {
        pool.reset(id);
        await pool.connect(id);
      });
    });
    const run = (pid: string) => get().runs[pid] ?? emptyRun();
    await be.listen<{ projectId: string; text: string }>("ship-log", ({ projectId, text }) =>
      set((s) => ({ runs: { ...s.runs, [projectId]: { ...run(projectId), log: (run(projectId).log + text).slice(-120_000) } } })),
    );
    await be.listen<{ projectId: string; step: string; branch?: string | null; ok?: boolean | null }>("ship-progress", ({ projectId, ...p }) =>
      set((s) => ({ runs: { ...s.runs, [projectId]: { ...run(projectId), steps: [...run(projectId).steps, p] } } })),
    );
    await be.listen<{ projectId: string; outcome?: ShipOutcome }>("ship-done", ({ projectId, outcome }) =>
      set((s) => ({ runs: { ...s.runs, [projectId]: { ...run(projectId), running: false, outcome: outcome ?? run(projectId).outcome } } })),
    );
    put(await invoke<ProjectCfg[]>("sync_configs").catch(() => []));
    set({ others: await invoke<OtherSession[]>("sync_others").catch(() => []) });
    const st = await invoke<SyncStatus>("sync_status").catch(() => null);
    if (st) set({ status: st });
  },
  async openEditor(projectId, line) {
    try {
      const text = await invoke<string>("sync_config_text", { projectId });
      set({ editor: { projectId, text, line } });
    } catch (e) {
      useFleet.getState().showToast(String(e), "error");
    }
  },
}));

/** Sync is on for this project (fleet.yml sync.enabled and an origin remote). */
export function isSynced(projectId: string | null | undefined): boolean {
  return !!projectId && !!useSync.getState().configs[projectId]?.enabled;
}

/** The project the Ship sheet / ⌘⇧D acts on: the selected session's, else the first synced. */
export function shipProject(): string | null {
  const st = useFleet.getState();
  const cur = st.activeSessionId ? st.sessions[st.activeSessionId]?.projectId : st.editorProjectId;
  if (cur && isSynced(cur)) return cur;
  if (cur) {
    // Never ship another project than the one you're in: set this one up instead (it can
    // create its GitHub repo from there).
    useSync.setState({ setup: cur });
    return null;
  }
  return st.projects.find((p) => isSynced(p.id))?.id ?? null;
}

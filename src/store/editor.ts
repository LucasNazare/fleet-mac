import { create } from "zustand";
import { useFleet } from "./store";

export type EditorTab =
  | { id: string; kind: "file"; projectId: string; path: string }
  | { id: string; kind: "diff"; projectId: string; root: string; path: string; staged: boolean }
  | { id: string; kind: "commit"; projectId: string; root: string; hash: string; path: string }
  | { id: string; kind: "conflict"; projectId: string; root: string; path: string };

type DistributiveOmit<T, K extends keyof EditorTab> = T extends EditorTab ? Omit<T, K> : never;
export type NewTab = DistributiveOmit<EditorTab, "id"> & { id?: string };

interface EditorStore {
  tabs: EditorTab[];
  active: Record<string, string | null>;
  /** Unsaved state per absolute path. */
  dirty: Record<string, boolean>;
  open(tab: NewTab): void;
  close(id: string): void;
  setDirty(path: string, d: boolean): void;
}

export function tabId(t: NewTab): string {
  switch (t.kind) {
    case "file": return `file:${t.path}`;
    case "diff": return `diff:${t.staged ? "s" : "w"}:${t.root}:${t.path}`;
    case "commit": return `commit:${t.hash}:${t.path}`;
    case "conflict": return `conflict:${t.root}:${t.path}`;
  }
}

export const useEditor = create<EditorStore>((set, get) => ({
  tabs: [],
  active: {},
  dirty: {},
  open(t) {
    const id = t.id ?? tabId(t);
    const tab = { ...t, id } as EditorTab;
    set((s) => {
      // A diff of the same file replaces the other side's diff tab in place.
      const existing = s.tabs.findIndex((x) => x.id === id);
      const tabs = existing >= 0 ? s.tabs : [...s.tabs, tab];
      return { tabs, active: { ...s.active, [tab.projectId]: id } };
    });
    useFleet.getState().setUi({ centerTab: "editor", editorProjectId: tab.projectId });
    void get();
  },
  close(id) {
    set((s) => {
      const t = s.tabs.find((x) => x.id === id);
      const tabs = s.tabs.filter((x) => x.id !== id);
      const active = { ...s.active };
      if (t && active[t.projectId] === id) {
        const same = tabs.filter((x) => x.projectId === t.projectId);
        active[t.projectId] = same.length ? same[same.length - 1].id : null;
      }
      return { tabs, active };
    });
  },
  setDirty(path, d) {
    set((s) => (s.dirty[path] === d ? s : { dirty: { ...s.dirty, [path]: d } }));
  },
}));

export function tabTitle(t: EditorTab): string {
  const base = t.path.split("/").pop() ?? t.path;
  switch (t.kind) {
    case "file": return base;
    case "diff": return `${base} (${t.staged ? "staged" : "changes"})`;
    case "commit": return `${base} @ ${t.hash.slice(0, 7)}`;
    case "conflict": return `${base} (conflict)`;
  }
}

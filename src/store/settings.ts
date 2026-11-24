import { create } from "zustand";
import { invoke } from "../ipc";
import { PALETTE } from "../theme";
import type { Effects, ThemeId } from "../themes/types";

export interface Settings {
  appearance: { theme: ThemeId; effects: Effects; uiFontSize: number; terminalFontSize: number; palette: string[] };
  claude: { path: string; defaultArgs: string; permissionMode: string; model: string; autoTitles: boolean };
  terminal: { scrollback: number; cursorStyle: "block" | "bar" | "underline"; copyOnSelect: boolean; optionAsMeta: boolean };
  editor: { tabSize: number; fontSize: number; formatOnSave: Record<string, boolean>; lspServers: Record<string, string[]> };
  git: { onlySessionFiles: boolean; autoFetchMinutes: number };
  notifications: { mutedProjects: string[]; mutedSessions: string[]; menuBar: boolean; sounds: boolean };
  /** Command id → accelerator, e.g. "Mod+K". Empty = default. */
  shortcuts: Record<string, string>;
}

export const DEFAULT_SETTINGS: Settings = {
  appearance: { theme: "dark", effects: "full", uiFontSize: 12.5, terminalFontSize: 13, palette: PALETTE },
  claude: { path: "", defaultArgs: "", permissionMode: "default", model: "haiku", autoTitles: true },
  terminal: { scrollback: 10000, cursorStyle: "block", copyOnSelect: false, optionAsMeta: true },
  editor: { tabSize: 2, fontSize: 13, formatOnSave: {}, lspServers: {} },
  git: { onlySessionFiles: false, autoFetchMinutes: 5 },
  notifications: { mutedProjects: [], mutedSessions: [], menuBar: false, sounds: false },
  shortcuts: {},
};

/** Deep-merge saved settings over defaults so new keys get defaults. */
export function mergeSettings(saved: unknown): Settings {
  const s = (saved && typeof saved === "object" ? saved : {}) as Record<string, Record<string, unknown>>;
  const out = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, Record<string, unknown>>;
  for (const k of Object.keys(out)) {
    if (s[k] && typeof s[k] === "object") out[k] = { ...out[k], ...s[k] };
  }
  return out as unknown as Settings;
}

interface SettingsStore {
  settings: Settings;
  loaded: boolean;
  load(): Promise<void>;
  update(fn: (s: Settings) => void): void;
  formatOnSave(projectId: string): boolean;
}

let saveTimer: number | undefined;

export const useSettings = create<SettingsStore>((set, get) => ({
  settings: DEFAULT_SETTINGS,
  loaded: false,
  async load() {
    const saved = await invoke<unknown>("get_settings").catch(() => null);
    set({ settings: mergeSettings(saved), loaded: true });
  },
  update(fn) {
    const next = structuredClone(get().settings);
    fn(next);
    set({ settings: next });
    window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => void invoke("save_settings", { settings: next }).catch(() => {}), 400);
  },
  formatOnSave(projectId) {
    return !!get().settings.editor.formatOnSave[projectId];
  },
}));

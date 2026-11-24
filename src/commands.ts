// Every app action, with its default shortcut. Shortcuts, the ⌘K palette and the
// settings shortcut editor all use this registry. App shortcuts always include ⌘;
// Ctrl combinations are never taken (they belong to the terminal).
import { useSync, shipProject } from "./store/sync";
import { THEMES } from "./themes/defs";
import { useFleet, orderedProjects, orderedSessions, projectSessions } from "./store/store";
import { useSettings } from "./store/settings";
import { useEditor } from "./store/editor";
import { useGit } from "./store/git";
import { needsYou } from "./theme";
import { toggleSplit } from "./components/CenterPane";
import { openSendPicker } from "./components/SendPicker";
import { restartProcess, stopProcess } from "./components/ProcessRows";
import { useQuickOpen } from "./components/editor/QuickOpen";
import { closeTab } from "./components/editor/EditorPane";
import { activeEditor, relPath } from "./editor/active";
import { invoke } from "./ipc";
import * as pool from "./terminal/termPool";
import { openUsage } from "./components/Usage";

export interface Command {
  id: string;
  title: string;
  keys?: string;
  /** Return false when not applicable, so the key falls through (to Monaco, the terminal…). */
  run: () => boolean | void;
  /** Hidden from the palette (context-only actions). */
  hidden?: boolean;
}

export function focusedSessionId() {
  const st = useFleet.getState();
  return st.splitSessionId && st.splitSessionId !== "__editor__" && st.focusedPane === 1 ? st.splitSessionId : st.activeSessionId;
}

/** The process a ⌘R / ⌘. should act on: a focused process row, else the open process terminal. */
function targetProcess(): string | null {
  const el = document.activeElement as HTMLElement | null;
  const row = el?.closest?.("[data-proc]") as HTMLElement | null;
  if (row?.dataset.proc) return row.dataset.proc;
  const st = useFleet.getState();
  return st.centerTab === "process" ? st.selectedProcess : null;
}

function terminalFocused() {
  return !!(document.activeElement as HTMLElement | null)?.closest?.(".xterm");
}
function editorFocused() {
  return !!(document.activeElement as HTMLElement | null)?.closest?.(".monaco-editor");
}

/** Terminal that has the user's attention (for ⌘⇧S). */
function focusedTerminalId(): string | null {
  const st = useFleet.getState();
  if (st.centerTab === "process") return st.selectedProcess;
  return focusedSessionId();
}

function step(dir: 1 | -1) {
  const st = useFleet.getState();
  const list = orderedSessions(st.projects, st.sessions);
  if (!list.length) return;
  const i = list.findIndex((s) => s.id === focusedSessionId());
  st.select(list[(i + dir + list.length) % list.length].id);
}

function stepProject(dir: 1 | -1) {
  const st = useFleet.getState();
  const ps = orderedProjects(st.projects).filter((p) => projectSessions(p.id, st.sessions).length);
  if (!ps.length) return;
  const cur = focusedSessionId() ? st.sessions[focusedSessionId()!]?.projectId : null;
  const i = ps.findIndex((p) => p.id === cur);
  const next = ps[(i + dir + ps.length) % ps.length];
  st.select(projectSessions(next.id, st.sessions)[0].id);
}

export function jumpToNeedsYou() {
  const st = useFleet.getState();
  const waiting = Object.values(st.sessions).filter((s) => !s.archived && needsYou(s.state)).sort((a, b) => a.stateSince - b.stateSince);
  if (!waiting.length) return st.showToast("No session needs you right now.");
  const cur = focusedSessionId();
  const i = waiting.findIndex((s) => s.id === cur);
  st.select(waiting[(i + 1) % waiting.length].id);
}

function askAboutThis() {
  const a = activeEditor();
  const st = useFleet.getState();
  const sid = st.activeSessionId;
  if (!a || !sid) return false;
  const m = a.editor.getModel()!;
  const sel = a.editor.getSelection()!;
  const s = sel.startLineNumber;
  const e = sel.isEmpty() ? s : sel.endLineNumber;
  const text = sel.isEmpty() ? m.getLineContent(s) : m.getValueInRange(sel);
  const msg = `About ${relPath(a.root, a.path)} lines ${s}–${e}:\n\`\`\`\n${text}\n\`\`\`\n`;
  void invoke("send_to_session", { sessionId: sid, text: msg }).then(() => st.select(sid));
  return true;
}

function revealLatestEdit() {
  const st = useFleet.getState();
  const s = st.activeSessionId ? st.sessions[st.activeSessionId] : null;
  const last = s?.filesTouched[s.filesTouched.length - 1];
  if (!s || !last) return st.showToast("This session hasn't edited a file yet.");
  useEditor.getState().open({ kind: "file", projectId: s.projectId, path: last });
}

function closeSomething() {
  const st = useFleet.getState();
  if (st.centerTab === "editor" || (st.splitSessionId === "__editor__" && st.focusedPane === 1)) {
    const pid = st.editorProjectId ?? (st.activeSessionId ? st.sessions[st.activeSessionId]?.projectId : null) ?? st.projects[0]?.id;
    const ed = useEditor.getState();
    const t = ed.tabs.find((x) => x.id === ed.active[pid ?? ""]);
    if (t) closeTab(t);
    return;
  }
  const id = focusedSessionId();
  if (id && st.sessions[id]) useFleet.setState({ confirm: { session: st.sessions[id] } });
}

/** ⌃N: open (or focus, or hide) the console under the focused session. */
export function toggleConsole() {
  const st = useFleet.getState();
  const sid = focusedSessionId();
  if (!sid || !st.sessions[sid]) return st.showToast("Select a session first.");
  if (st.centerTab !== "session") st.setUi({ centerTab: "session" });
  const c = st.consoles[sid];
  const focused = !!(document.activeElement as HTMLElement | null)?.closest?.(".console-pane");
  if (c?.open && focused) {
    useFleet.setState({ consoles: { ...st.consoles, [sid]: { ...c, open: false } } });
    pool.focus(sid);
    return;
  }
  if (c?.open && c.active) return pool.focus(c.active);
  if (c?.tabs.length) {
    useFleet.setState({ consoles: { ...st.consoles, [sid]: { ...c, open: true } } });
    return;
  }
  void openConsoleTab(sid);
}

export async function openConsoleTab(sid: string) {
  const st = useFleet.getState();
  try {
    const { cols } = pool.currentSize();
    const id = await invoke<string>("console_open", { sessionId: sid, cols, rows: 12 });
    const cur = useFleet.getState().consoles[sid];
    useFleet.setState({ consoles: { ...useFleet.getState().consoles, [sid]: { open: true, height: cur?.height ?? 240, tabs: [...(cur?.tabs ?? []), id], active: id } } });
  } catch (e) {
    st.showToast(String(e), "error");
  }
}

function currentProjectId(): string | null {
  const st = useFleet.getState();
  return (st.activeSessionId ? st.sessions[st.activeSessionId]?.projectId : null) ?? st.editorProjectId ?? st.projects[0]?.id ?? null;
}

export const COMMANDS: Command[] = [
  { id: "palette", title: "Command palette", keys: "Mod+K", run: () => useFleet.setState({ paletteOpen: true }) },
  { id: "githubSync", title: "GitHub sync for this project…", run: () => { const p = currentProjectId(); if (!p) return false; useSync.setState({ setup: p }); } },
  { id: "editFleetYml", title: "Edit fleet.yml…", run: () => { const p = currentProjectId(); if (!p) return false; void useSync.getState().openEditor(p); } },
  { id: "syncNow", title: "Sync with GitHub now", run: () => void invoke("sync_now") },
  { id: "ship", title: "Ship to main…", keys: "Mod+Shift+D", run: () => { const p = shipProject(); if (p) useSync.setState({ ship: p }); } },
  { id: "addFromGithub", title: "Add project from GitHub…", run: () => useSync.setState({ reposOpen: true }) },
  ...THEMES.map((t) => ({
    id: `theme.${t.id}`,
    title: `Theme: ${t.name}`,
    run: () => void import("./store/settings").then(({ useSettings }) => useSettings.getState().update((s) => { s.appearance.theme = t.id; })),
  })),
  { id: "pauseSync", title: "Pause / resume GitHub sync (all projects)", run: () => void invoke("sync_set_paused", { projectId: null, paused: !useSync.getState().status.pausedAll }) },
  { id: "jumpNeedsYou", title: "Jump to next session that needs you", keys: "Mod+J", run: jumpToNeedsYou },
  { id: "nextSession", title: "Next session", keys: "Mod+]", run: () => step(1) },
  { id: "prevSession", title: "Previous session", keys: "Mod+[", run: () => step(-1) },
  { id: "nextProject", title: "Next project", keys: "Mod+Alt+]", run: () => stepProject(1) },
  { id: "prevProject", title: "Previous project", keys: "Mod+Alt+[", run: () => stepProject(-1) },
  { id: "newSession", title: "New session (current project)", keys: "Mod+N", run: () => void useFleet.getState().quickSession() },
  { id: "newSessionOptions", title: "New session with prompt / worktree…", keys: "Mod+Shift+N", run: () => useFleet.getState().setUi({ sheetOpen: true, sheetProjectId: null }) },
  { id: "centerSession", title: "Show session", keys: "Mod+1", run: () => useFleet.getState().setUi({ centerTab: "session" }) },
  { id: "centerEditor", title: "Show editor", keys: "Mod+2", run: () => useFleet.getState().setUi({ centerTab: "editor" }) },
  { id: "centerProcess", title: "Show process", keys: "Mod+3", run: () => useFleet.getState().setUi({ centerTab: "process" }) },
  { id: "toggleEditor", title: "Toggle editor", keys: "Mod+E", run: () => { const s = useFleet.getState(); s.setUi({ centerTab: s.centerTab === "editor" ? "session" : "editor" }); } },
  { id: "split", title: "Split center pane", keys: "Mod+\\", run: () => toggleSplit() },
  { id: "toggleLeft", title: "Hide/show left panel", keys: "Mod+B", run: () => { const s = useFleet.getState(); s.setUi({ leftHidden: !s.leftHidden }); } },
  { id: "toggleGit", title: "Hide/show the git panel", keys: "Mod+G", run: () => { const s = useFleet.getState(); s.setUi({ rightHidden: !s.rightHidden }); } },
  { id: "toggleRight", title: "Hide/show right panel", keys: "Mod+Alt+B", hidden: true, run: () => { const s = useFleet.getState(); s.setUi({ rightHidden: !s.rightHidden }); } },
  { id: "focusGit", title: "Focus git panel", keys: "Mod+Shift+G", run: () => { const s = useFleet.getState(); if (s.rightHidden) s.setUi({ rightHidden: false }); setTimeout(() => window.dispatchEvent(new Event("fleet:focus-git")), 0); } },
  { id: "showSessions", title: "Show session list", keys: "Mod+Shift+1", run: () => useFleet.getState().setUi({ sidebarMode: "sessions", leftHidden: false }) },
  { id: "showFiles", title: "Show file tree", run: () => useFleet.getState().setUi({ sidebarMode: "files", leftHidden: false }) },
  { id: "quickOpen", title: "Quick open file", keys: "Mod+P", run: () => useQuickOpen.setState({ open: true }) },
  { id: "findInFiles", title: "Find in files", keys: "Mod+Shift+F", run: () => { useFleet.getState().setUi({ sidebarMode: "search", leftHidden: false }); setTimeout(() => window.dispatchEvent(new Event("fleet:find-in-files")), 0); } },
  { id: "inlineEdit", title: "Inline edit (⌘I)", keys: "Mod+I", run: () => { const a = activeEditor(); if (!a) return false; a.inlineEdit(); } },
  { id: "askAbout", title: "Ask the session about this selection", keys: "Mod+Shift+I", run: askAboutThis },
  { id: "revealLatestEdit", title: "Reveal the session's latest edited file", keys: "Mod+Shift+E", run: revealLatestEdit },
  { id: "sendToSession", title: "Send selected text to a session", keys: "Mod+Shift+S", run: () => { const id = focusedTerminalId(); if (!id) return false; openSendPicker(id); } },
  { id: "toggleConsole", title: "Toggle the session's console", keys: "Ctrl+N", run: () => toggleConsole() },
  { id: "close", title: "Close editor tab / session", keys: "Mod+W", run: closeSomething },
  { id: "settings", title: "Settings", keys: "Mod+,", run: () => useFleet.setState({ settingsOpen: true }) },
  { id: "usage", title: "Show usage (cost & turn time)", run: () => openUsage() },
  { id: "save", title: "Save file", keys: "Mod+S", hidden: true, run: () => { const a = activeEditor(); if (!a) return false; void a.save(); } },
  { id: "gotoLine", title: "Go to line", keys: "Ctrl+G", run: () => { const a = activeEditor(); if (!a || !editorFocused()) return false; a.editor.focus(); a.editor.trigger("fleet", "editor.action.gotoLine", null); } },
  { id: "restartProcess", title: "Restart process", keys: "Mod+R", run: () => { const k = targetProcess(); if (!k) return false; const [, p, d] = k.split(":"); void restartProcess(p, d); } },
  { id: "stopProcess", title: "Stop process", keys: "Mod+.", run: () => { const k = targetProcess(); if (!k) return false; const [, p, d] = k.split(":"); void stopProcess(p, d); } },
  { id: "findInTerminal", title: "Find in process output", keys: "Mod+F", hidden: true, run: () => { if (useFleet.getState().centerTab !== "process" || editorFocused()) return false; window.dispatchEvent(new Event("fleet:find")); } },
  { id: "copyTerminal", title: "Copy terminal selection", keys: "Mod+C", hidden: true, run: () => {
    if (!terminalFocused()) return false;
    const id = focusedTerminalId();
    const sel = id ? pool.hasSelection(id) : null;
    if (!sel) return false;
    void navigator.clipboard.writeText(sel);
  } },
  { id: "fetch", title: "Git: fetch", run: () => gitOp("fetch") },
  { id: "pull", title: "Git: pull", run: () => gitOp("pull") },
  { id: "push", title: "Git: push", run: () => gitOp("push") },
  { id: "addProject", title: "Add project folder…", run: () => void useFleet.getState().addProject() },
];

function gitOp(op: string) {
  void import("./store/git").then(({ currentRoot }) => {
    const r = currentRoot();
    if (r) void useGit.getState().act(r.root, op, () => invoke("git_remote", { root: r.root, op }));
  });
}

// ---------------------------------------------------------------- key handling

const CODE: Record<string, string> = {
  BracketRight: "]", BracketLeft: "[", Backslash: "\\", Period: ".", Comma: ",", Slash: "/", Semicolon: ";",
  Quote: "'", Minus: "-", Equal: "=", Backquote: "`", Enter: "Enter", Escape: "Escape", Space: "Space", Tab: "Tab",
};

/** "Mod+Ctrl+Alt+Shift+K" for a keydown (Mod = ⌘). Only combos a command is bound to are taken,
 *  so Ctrl keys still reach the terminal unless a command claims one (⌃N = console). */
export function accelOf(e: KeyboardEvent): string | null {
  if (!e.metaKey && !e.altKey && !e.ctrlKey) return null;
  let key = e.code.startsWith("Key") ? e.code.slice(3) : e.code.startsWith("Digit") ? e.code.slice(5) : CODE[e.code] ?? (/^F\d+$/.test(e.code) ? e.code : null);
  if (!key) return null;
  return [e.metaKey ? "Mod" : "", e.ctrlKey ? "Ctrl" : "", e.altKey ? "Alt" : "", e.shiftKey ? "Shift" : "", key].filter(Boolean).join("+");
}

export function prettyKeys(a?: string) {
  if (!a) return "";
  return a.replace("Mod+", "⌘").replace("Ctrl+", "⌃").replace("Alt+", "⌥").replace("Shift+", "⇧").replace(/\+/g, "");
}

export function keysFor(id: string): string | undefined {
  const custom = useSettings.getState().settings.shortcuts[id];
  return custom === "" ? undefined : custom ?? COMMANDS.find((c) => c.id === id)?.keys;
}

/** Other commands bound to the same keys. */
export function clashes(id: string, keys: string): Command[] {
  return COMMANDS.filter((c) => c.id !== id && keysFor(c.id) === keys);
}

let recording = false;
/** While the settings shortcut recorder listens, app shortcuts are paused. */
export function setRecordingShortcut(on: boolean) {
  recording = on;
}

export function handleKey(e: KeyboardEvent): boolean {
  if (recording) return false;
  const a = accelOf(e);
  if (!a) return false;
  for (const c of COMMANDS) {
    if (keysFor(c.id) !== a) continue;
    if (c.run() === false) continue;
    e.preventDefault();
    e.stopPropagation();
    return true;
  }
  return false;
}

export function runCommand(id: string) {
  COMMANDS.find((c) => c.id === id)?.run();
}

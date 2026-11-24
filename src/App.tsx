import { useEffect, useRef } from "react";
import { useFleet } from "./store/store";
import { useGit } from "./store/git";
import { useSettings } from "./store/settings";
import { useEditor } from "./store/editor";
import { TopBar } from "./components/TopBar";
import { Sidebar } from "./components/Sidebar";
import { CenterPane } from "./components/CenterPane";
import { StatusBar } from "./components/StatusBar";
import { NewSessionSheet } from "./components/NewSessionSheet";
import { ConfirmClose } from "./components/ConfirmClose";
import { ProcessForm } from "./components/ProcessForm";
import { SendPicker } from "./components/SendPicker";
import { QuitDialog } from "./components/QuitDialog";
import { Dialogs } from "./components/Dialogs";
import { UsageSheet } from "./components/Usage";
import { GitPanel } from "./components/git/GitPanel";
import { QuickOpen } from "./components/editor/QuickOpen";
import { Palette } from "./components/Palette";
import { SettingsView } from "./components/SettingsView";
import { SshForm } from "./components/RemoteRows";
import { SyncSetupDialog, ConfigEditor } from "./components/SyncUi";
import { ShipSheet, ReposDialog } from "./components/ShipUi";
import { useSync } from "./store/sync";
import { handleKey } from "./commands";
import { backend, invoke } from "./ipc";
import { STATE } from "./theme";
import * as pool from "./terminal/termPool";
import { currentRoot } from "./store/git";
import { applyTheme } from "./themes";

let appStarted = false;

function Resizer({ side }: { side: "left" | "right" }) {
  const onDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const st = useFleet.getState();
    const startX = e.clientX;
    const start = side === "left" ? st.leftW : st.rightW;
    const move = (ev: MouseEvent) => {
      const d = ev.clientX - startX;
      const w = Math.max(200, Math.min(640, side === "left" ? start + d : start - d));
      useFleet.setState(side === "left" ? { leftW: w } : { rightW: w });
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      document.body.classList.remove("resizing");
      useFleet.getState().setUi({});
    };
    document.body.classList.add("resizing");
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };
  return <div className="resizer" onMouseDown={onDown} role="separator" aria-orientation="vertical" />;
}

export default function App() {
  const st = useFleet();
  const searchRef = useRef<HTMLInputElement>(null);
  const settings = useSettings((s) => s.settings);

  useEffect(() => {
    if (appStarted) return; // StrictMode runs this twice in dev
    appStarted = true;
    void (async () => {
      await useSettings.getState().load();
      await useGit.getState().init();
      await st.init();
      await useSync.getState().init();
      // LSP "go to definition" into another file opens it in a fleet tab.
      const { monaco } = await import("./editor/monaco");
      monaco.editor.registerEditorOpener({
        openCodeEditor(_src, resource, sel) {
          const path = resource.fsPath;
          const fleet = useFleet.getState();
          const project = fleet.projects.find((p) => path.startsWith(p.rootPath + "/"));
          if (!project) return false;
          useEditor.getState().open({ kind: "file", projectId: project.id, path });
          const t0 = Date.now();
          const reveal = () => {
            const ed = monaco.editor.getEditors().find((e) => e.getModel()?.uri.fsPath === path);
            if (!ed) return Date.now() - t0 < 2000 && void setTimeout(reveal, 40);
            if (sel && "startLineNumber" in sel) { ed.setSelection(sel); ed.revealRangeInCenter(sel); }
            else if (sel) { ed.setPosition(sel); ed.revealPositionInCenter(sel); }
            ed.focus();
          };
          setTimeout(reveal, 0);
          return true;
        },
      });
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ⌘ shortcuts from the command registry (capture phase, before Monaco / xterm).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => void handleKey(e);
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  // Appearance + terminal settings.
  useEffect(() => {
    document.documentElement.style.setProperty("--fs", `${settings.appearance.uiFontSize}px`);
    pool.applySettings(settings.terminal, settings.appearance.terminalFontSize);
  }, [settings]);
  // Theme: CSS tokens, fonts, terminals, editor and the native window.
  useEffect(() => {
    void applyTheme(settings.appearance.theme, settings.appearance.effects ?? "full", (w) => void invoke("set_window_appearance", w).catch(() => {}));
  }, [settings.appearance.theme, settings.appearance.effects]);

  // Auto-fetch the current checkout.
  useEffect(() => {
    const mins = settings.git.autoFetchMinutes;
    if (!mins) return;
    const t = window.setInterval(() => {
      const r = currentRoot();
      if (r && useGit.getState().repos[r.root]?.status?.isRepo) void invoke("git_remote", { root: r.root, op: "fetch" }).catch(() => {});
    }, mins * 60_000);
    return () => window.clearInterval(t);
  }, [settings.git.autoFetchMinutes]);

  // Window title: "● PX · Pix fee labels — atrium-api"
  const active = st.activeSessionId ? st.sessions[st.activeSessionId] : null;
  const activeProject = active ? st.projects.find((p) => p.id === active.projectId) : null;
  const titleText = active ? `${STATE[active.state].symbol} ${active.glyph} · ${active.title} — ${activeProject?.name ?? ""}` : "fleet";
  useEffect(() => {
    void backend().then((b) => b.setTitle(titleText));
  }, [titleText]);

  // A process (re)started under the same terminal id: re-attach its output.
  const startedRef = useRef<Record<string, number>>({});
  useEffect(() => {
    for (const r of Object.values(st.runs)) {
      const prev = startedRef.current[r.key];
      if (r.status === "running" && prev !== undefined && prev !== r.startedAt) {
        void pool.reconnect(r.key, `started ${new Date(r.startedAt).toLocaleTimeString()}`);
      }
      startedRef.current[r.key] = r.startedAt;
    }
  }, [st.runs]);

  // Tell the backend what's on screen so it can skip redundant notifications.
  const visibleKey = [
    st.centerTab === "session" ? st.activeSessionId : null,
    st.centerTab === "session" && st.splitSessionId !== "__editor__" ? st.splitSessionId : null,
    st.centerTab === "process" ? st.selectedProcess : null,
  ].filter(Boolean).join(",");
  useEffect(() => {
    void invoke("set_visible", { ids: visibleKey ? visibleKey.split(",") : [], focused: document.hasFocus() }).catch(() => {});
  }, [visibleKey]);

  // Persist open editor tabs per project (restored on launch).
  const tabs = useEditor((s) => s.tabs);
  const activeTabs = useEditor((s) => s.active);
  useEffect(() => {
    if (!st.ready) return;
    const t = window.setTimeout(() => void invoke("save_ui", { ui: { ...uiPrefs(), editorTabs: tabs.filter((x) => x.kind === "file"), editorActive: activeTabs } }).catch(() => {}), 800);
    return () => window.clearTimeout(t);
  }, [tabs, activeTabs, st.ready]);

  const cols = [st.leftHidden ? "" : `${st.leftW}px 4px`, "minmax(320px, 1fr)", st.rightHidden ? "" : `4px ${st.rightW}px`].filter(Boolean).join(" ");

  return (
    <div className="app">
      <TopBar />
      {st.warnings.length > 0 && <div className="warn-bar">{st.warnings.map((w) => <div key={w}>⚠ {w}</div>)}</div>}
      <div className="main" style={{ gridTemplateColumns: cols }}>
        {!st.leftHidden && <Sidebar searchRef={searchRef} />}
        {!st.leftHidden && <Resizer side="left" />}
        <CenterPane />
        {!st.rightHidden && <Resizer side="right" />}
        {!st.rightHidden && <GitPanel />}
      </div>
      <StatusBar />
      {st.sheetOpen && <NewSessionSheet />}
      <ConfirmClose />
      {st.procForm && <ProcessForm key={`${st.procForm.projectId}-${st.procForm.def?.id ?? "new"}`} />}
      <SendPicker />
      <QuitDialog />
      <QuickOpen />
      {st.paletteOpen && <Palette />}
      {st.settingsOpen && <SettingsView />}
      {st.sshForm && <SshForm key={st.sshForm.conn?.id ?? `new-${st.sshForm.projectId}`} />}
      <SyncSetupDialog />
      <ShipSheet />
      <ReposDialog />
      <ConfigEditor />
      <UsageSheet />
      <Dialogs />
      {st.toast && <div className={`toast ${st.toast.kind}`} role="status">{st.toast.text}</div>}
    </div>
  );
}

function uiPrefs() {
  const s = useFleet.getState();
  return { leftW: s.leftW, rightW: s.rightW, leftHidden: s.leftHidden, rightHidden: s.rightHidden, activeSessionId: s.activeSessionId };
}

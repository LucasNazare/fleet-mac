import { memo } from "react";
import { useFleet, type CenterTab } from "../store/store";
import { SessionView } from "./SessionView";
import { ProcessView } from "./ProcessView";
import { EditorPane } from "./editor/EditorPane";

const TABS: { id: CenterTab; label: string; key: string }[] = [
  { id: "session", label: "Session", key: "⌘1" },
  { id: "editor", label: "Editor", key: "⌘2" },
  { id: "process", label: "Process", key: "⌘3" },
];

export const CenterPane = memo(function CenterPane() {
  const { centerTab, sessions, activeSessionId, splitSessionId, focusedPane, projects, setUi, addProject } = useFleet();
  const a = activeSessionId ? sessions[activeSessionId] : null;
  const editorSplit = splitSessionId === "__editor__";
  const b = splitSessionId && !editorSplit ? sessions[splitSessionId] : null;

  return (
    <main className="center">
      <nav className="center-tabs">
        {TABS.map((t) => (
          <button key={t.id} className={`tab ${centerTab === t.id ? "on" : ""}`} onClick={() => setUi({ centerTab: t.id })}>
            {t.label} <span className="muted small">{t.key}</span>
          </button>
        ))}
        <span className="spacer" />
        <button className="tab" onClick={() => toggleSplit()} title="Split the center pane (⌘\)">
          {splitSessionId ? "Unsplit" : "Split"} <span className="muted small">⌘\</span>
        </button>
      </nav>
      <div className="center-body">
        {centerTab === "session" && (
          <div className={`panes ${b || editorSplit ? "split" : ""}`}>
            {a ? (
              <SessionView key={`p0-${a.id}`} s={a} pane={0} focused={focusedPane === 0 || (!b && !editorSplit)} />
            ) : (
              <div className="empty-center">
                {projects.length === 0 ? (
                  <>
                    <h2>Add a project to start</h2>
                    <p className="muted">fleet groups Claude Code sessions by project folder.</p>
                    <button className="btn primary" onClick={() => void addProject()}>Add project folder</button>
                  </>
                ) : (
                  <>
                    <h2>No session selected</h2>
                    <p className="muted">Start one with <kbd>⌘N</kbd> or pick one on the left.</p>
                    <button className="btn primary" onClick={() => void useFleet.getState().quickSession()}>New session</button>
                  </>
                )}
              </div>
            )}
            {b && <SessionView key={`p1-${b.id}`} s={b} pane={1} focused={focusedPane === 1} />}
            {editorSplit && <div className="editor-split" onMouseDown={() => useFleet.setState({ focusedPane: 1 })}><EditorPane /></div>}
          </div>
        )}
        {centerTab === "editor" && <EditorPane />}
        {centerTab === "process" && <ProcessView />}
      </div>
    </main>
  );
});

export function toggleSplit() {
  const st = useFleet.getState();
  if (st.splitSessionId) {
    const wasEditor = st.splitSessionId === "__editor__";
    st.setUi({ splitSessionId: null, focusedPane: 0, ...(wasEditor ? { centerTab: "editor" as const } : {}) });
    return;
  }
  // From the editor: session on the left, editor on the right.
  if (st.centerTab === "editor") {
    if (!st.activeSessionId) return st.showToast("Select a session to show beside the editor.");
    st.setUi({ splitSessionId: "__editor__", focusedPane: 1, centerTab: "session", sidebarMode: "files" });
    return;
  }
  const live = Object.values(st.sessions).filter((s) => !s.archived && s.id !== st.activeSessionId).sort((x, y) => y.stateSince - x.stateSince);
  if (!live[0]) {
    st.showToast("Start a second session to split the view.");
    return;
  }
  st.setUi({ splitSessionId: live[0].id, focusedPane: 1, centerTab: "session" });
}

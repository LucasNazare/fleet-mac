import { useFleet } from "../../store/store";
import { useEditor, tabTitle, type EditorTab } from "../../store/editor";
import { FileView } from "./FileView";
import { DiffView } from "./DiffView";
import { closeDoc, isDirty, saveDoc } from "../../editor/models";
import { confirmDialog } from "../Dialogs";

export function closeTab(t: EditorTab) {
  const close = () => {
    useEditor.getState().close(t.id);
    if (t.kind === "file" && !useEditor.getState().tabs.some((x) => x.kind === "file" && x.path === t.path)) closeDoc(t.path);
  };
  if (t.kind === "file" && isDirty(t.path)) {
    confirmDialog({
      title: `Save changes to ${t.path.split("/").pop()}?`,
      body: "Your changes will be lost if you don't save them.",
      confirmLabel: "Save",
      extra: { label: "Don't save", onClick: close },
      onConfirm: () => void saveDoc(t.path).then(close),
    });
  } else close();
}

export function EditorPane() {
  const { projects, editorProjectId, sessions, activeSessionId } = useFleet();
  const { tabs, active, dirty } = useEditor();
  const pid = editorProjectId ?? (activeSessionId ? sessions[activeSessionId]?.projectId : null) ?? projects[0]?.id ?? null;
  const project = projects.find((p) => p.id === pid);
  if (!project) return <div className="placeholder"><h2>Editor</h2><p className="muted">Add a project first.</p></div>;
  const mine = tabs.filter((t) => t.projectId === project.id);
  const cur = mine.find((t) => t.id === active[project.id]) ?? mine[mine.length - 1];

  return (
    <div className="editor-pane" data-testid="editor-pane">
      <div className="editor-tabs">
        <select className="proj-select" value={project.id} onChange={(e) => useFleet.getState().setUi({ editorProjectId: e.target.value })} aria-label="Project">
          {projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
        {mine.map((t) => (
          <div key={t.id} className={`etab ${t.id === cur?.id ? "on" : ""}`} onClick={() => useEditor.setState((s) => ({ active: { ...s.active, [project.id]: t.id } }))} onAuxClick={(e) => e.button === 1 && closeTab(t)} title={t.path}>
            <span className={`etab-kind k-${t.kind}`} />
            {tabTitle(t)}
            {t.kind === "file" && dirty[t.path] ? <span className="dot" aria-label="unsaved">●</span> : null}
            <button className="etab-x" aria-label={`Close ${tabTitle(t)}`} onClick={(e) => { e.stopPropagation(); closeTab(t); }}>×</button>
          </div>
        ))}
      </div>
      <div className="editor-body">
        {!cur ? (
          <div className="placeholder">
            <h2>{project.name}</h2>
            <p className="muted"><kbd>⌘P</kbd> open a file · <kbd>⌘⇧F</kbd> find in files · pick a file in the tree · click a change in the git panel</p>
          </div>
        ) : cur.kind === "file" ? (
          <FileView key="file" path={cur.path} root={project.rootPath} projectId={project.id} />
        ) : cur.kind === "conflict" ? (
          <FileView key={`conflict:${cur.path}`} path={`${cur.root}/${cur.path}`} root={project.rootPath} projectId={project.id} mode="conflict" gitRoot={cur.root} relGitPath={cur.path} />
        ) : cur.kind === "diff" ? (
          <DiffView key="diff" root={cur.root} path={cur.path} staged={cur.staged} projectId={project.id} />
        ) : (
          <DiffView key="commit" root={cur.root} path={cur.path} hash={cur.hash} projectId={project.id} />
        )}
      </div>
    </div>
  );
}

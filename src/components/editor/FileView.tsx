import { useEffect, useRef, useState } from "react";
import { monaco, editorTheme, editorFontFamily } from "../../editor/monaco";
import { openDoc, saveDoc, onDiskChange, takeTheirs, keepMine, theirsText, isDirty } from "../../editor/models";
import { ensureLsp } from "../../editor/lsp";
import { setActiveEditor, relPath } from "../../editor/active";
import { gutterDecorations, lineChanges } from "../../editor/gutter";
import { useFleet } from "../../store/store";
import { useEditor } from "../../store/editor";
import { useSettings } from "../../store/settings";
import { backend, invoke } from "../../ipc";
import { InlineEdit, owningSession } from "./InlineEdit";
import { parseConflicts, resolveConflict } from "./conflicts";
import { useGit } from "../../store/git";

const viewStates = new Map<string, monaco.editor.ICodeEditorViewState | null>();
let created: monaco.editor.IStandaloneCodeEditor | null = null;

export function FileView({ path, root, projectId, mode = "file", gitRoot, relGitPath }: { path: string; root: string; projectId: string; mode?: "file" | "conflict"; gitRoot?: string; relGitPath?: string }) {
  const host = useRef<HTMLDivElement>(null);
  const ed = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const deco = useRef<monaco.editor.IEditorDecorationsCollection | null>(null);
  const head = useRef<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [inline, setInline] = useState(false);
  const [conflictBar, setConflictBar] = useState<string | null>(null);
  const [conflicts, setConflicts] = useState(0);
  const sessions = useFleet((s) => s.sessions);
  const settings = useSettings((s) => s.settings);

  // Create the editor once per mount.
  useEffect(() => {
    const e = monaco.editor.create(host.current!, {
      theme: editorTheme(),
      fontFamily: editorFontFamily(),
      fontSize: settings.editor.fontSize,
      tabSize: settings.editor.tabSize,
      automaticLayout: true,
      glyphMargin: true,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      renderLineHighlight: "all",
      lineDecorationsWidth: 10,
      fixedOverflowWidgets: true,
      model: null,
    });
    ed.current = e;
    created = e;
    deco.current = e.createDecorationsCollection([]);
    return () => {
      const m = e.getModel();
      if (m) viewStates.set(m.uri.fsPath, e.saveViewState());
      e.dispose();
      if (created === e) created = null;
      setActiveEditor(null);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    ed.current?.updateOptions({ fontSize: settings.editor.fontSize, tabSize: settings.editor.tabSize });
  }, [settings.editor.fontSize, settings.editor.tabSize]);

  const refreshGutter = () => {
    const e = ed.current;
    const m = e?.getModel();
    if (!e || !m || head.current == null) {
      deco.current?.set([]);
      return;
    }
    const live = Object.values(useFleet.getState().sessions).filter((s) => !s.archived);
    deco.current!.set(gutterDecorations(lineChanges(head.current, m.getValue()), path, live, m.getLineCount()));
    if (mode === "conflict") setConflicts(parseConflicts(m.getLinesContent()).length);
  };

  // Load the model for this path.
  useEffect(() => {
    let cancelled = false;
    const e = ed.current!;
    const prev = e.getModel();
    if (prev) viewStates.set(prev.uri.fsPath, e.saveViewState());
    setError(null);
    setConflictBar(null);
    void openDoc(path).then(async (d) => {
      if (cancelled) return;
      if ("error" in d) {
        setError(d.error);
        e.setModel(null);
        return;
      }
      e.setModel(d.model);
      const vs = viewStates.get(path);
      if (vs) e.restoreViewState(vs);
      e.focus();
      head.current = await invoke<string | null>("head_text", { path }).catch(() => null);
      refreshGutter();
      void ensureLsp(root, d.model.getLanguageId(), useSettings.getState().settings.editor.lspServers);
    });
    const save = async () => {
      await saveDoc(path, { root, format: useSettings.getState().formatOnSave(projectId) });
      head.current = await invoke<string | null>("head_text", { path }).catch(() => head.current);
      refreshGutter();
    };
    setActiveEditor({ editor: e, path, root, projectId, save, inlineEdit: () => setInline(true) });
    let t: number | undefined;
    const sub = e.onDidChangeModelContent(() => {
      window.clearTimeout(t);
      t = window.setTimeout(refreshGutter, 250);
    });
    return () => {
      cancelled = true;
      sub.dispose();
      window.clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  // Sessions' line marks changed → recolour.
  useEffect(refreshGutter, [sessions]); // eslint-disable-line react-hooks/exhaustive-deps

  // Agent edits on disk: reload silently, or ask when there are local edits.
  useEffect(() => {
    let un: (() => void) | undefined;
    void backend().then((b) =>
      b.listen<{ root: string; paths: string[] }>("fs-changed", async (ev) => {
        if (!ev.paths.includes(path)) return;
        const r = await onDiskChange(path);
        if (r === "conflict") setConflictBar(owningSession(path)?.glyph ?? "Someone");
        if (r === "reloaded") refreshGutter();
      }).then((u) => (un = u)),
    );
    const onGit = () => void invoke<string | null>("head_text", { path }).then((h) => { head.current = h; refreshGutter(); });
    const unsub = useGit.subscribe((s, p) => { if (s.repos !== p.repos) onGit(); });
    return () => { un?.(); unsub(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  const markResolved = async () => {
    const e = ed.current!;
    if (parseConflicts(e.getModel()!.getLinesContent()).length) return;
    await saveDoc(path, {});
    if (gitRoot && relGitPath) await useGit.getState().act(gitRoot, "stage", () => invoke("git_stage", { root: gitRoot, paths: [relGitPath] }));
    useEditor.getState().close(useEditor.getState().active[projectId] ?? "");
  };

  return (
    <div className="file-view">
      {mode === "conflict" && (
        <div className="conflict-bar" data-testid="conflict-bar">
          <span>{conflicts ? `${conflicts} conflict${conflicts === 1 ? "" : "s"} — the one at or after the cursor:` : "No conflict markers left."}</span>
          {conflicts > 0 && (["current", "incoming", "both"] as const).map((k) => (
            <button key={k} className="btn small" onClick={() => { resolveConflict(ed.current!, k); refreshGutter(); }}>Accept {k}</button>
          ))}
          <span className="spacer" />
          <button className="btn small primary" disabled={conflicts > 0} onClick={() => void markResolved()}>Mark resolved (stage)</button>
        </div>
      )}
      {conflictBar && (
        <div className="agent-bar" role="alert">
          <span>{conflictBar} changed this file. </span>
          <button className="btn small" onClick={() => { keepMine(path); setConflictBar(null); }}>Keep mine</button>
          <button className="btn small" onClick={() => { takeTheirs(path); setConflictBar(null); refreshGutter(); }}>Take theirs</button>
          <button className="btn small" onClick={() => {
            const theirs = theirsText(path) ?? "";
            const orig = monaco.editor.createModel(theirs, ed.current!.getModel()!.getLanguageId());
            const w = document.createElement("div");
            w.className = "compare-overlay";
            document.body.appendChild(w);
            const d = monaco.editor.createDiffEditor(w, { theme: editorTheme(), automaticLayout: true, readOnly: true, fontFamily: editorFontFamily() });
            d.setModel({ original: orig, modified: ed.current!.getModel()! });
            const close = document.createElement("button");
            close.textContent = "Close compare (theirs ← → mine)";
            close.className = "btn small compare-close";
            close.onclick = () => { d.dispose(); orig.dispose(); w.remove(); };
            w.appendChild(close);
          }}>Compare</button>
        </div>
      )}
      {error && <div className="placeholder"><p className="muted">{error}</p></div>}
      <div className="monaco-host" ref={host} style={{ display: error ? "none" : undefined }} />
      {inline && ed.current && <InlineEdit editor={ed.current} path={path} root={root} projectId={projectId} onClose={() => setInline(false)} />}
      <div className="file-status muted small">
        {relPath(root, path)}{isDirty(path) ? " · unsaved" : ""}
      </div>
    </div>
  );
}

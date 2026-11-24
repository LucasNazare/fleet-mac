import { useEffect, useRef, useState } from "react";
import { monaco, languageFor, editorTheme, editorFontFamily } from "../../editor/monaco";
import { invoke, backend } from "../../ipc";
import { useGit } from "../../store/git";
import { useEditor } from "../../store/editor";
import type { DiffResult, Hunk } from "../../types";
import { confirmDialog } from "../Dialogs";

/** Side-by-side diff with hunk and line staging (kind "diff") or a read-only commit diff (kind "commit"). */
export function DiffView({ root, path, staged, hash, projectId }: { root: string; path: string; staged?: boolean; hash?: string; projectId: string }) {
  const host = useRef<HTMLDivElement>(null);
  const de = useRef<monaco.editor.IStandaloneDiffEditor | null>(null);
  const [data, setData] = useState<DiffResult | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [cur, setCur] = useState(0);
  const [selLines, setSelLines] = useState<[number, number] | null>(null);
  const act = useGit((g) => g.act);
  const isCommit = !!hash;

  useEffect(() => {
    const d = monaco.editor.createDiffEditor(host.current!, {
      theme: editorTheme(),
      fontFamily: editorFontFamily(),
      fontSize: 13,
      automaticLayout: true,
      readOnly: true,
      originalEditable: false,
      renderSideBySide: true,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      renderOverviewRuler: true,
    });
    de.current = d;
    const mod = d.getModifiedEditor();
    const sub = mod.onDidChangeCursorSelection((e) => {
      const s = e.selection;
      setSelLines(s.isEmpty() ? null : [s.startLineNumber, s.endColumn === 1 && s.endLineNumber > s.startLineNumber ? s.endLineNumber - 1 : s.endLineNumber]);
    });
    const sub2 = mod.onDidChangeCursorPosition((e) => {
      setCur((c) => {
        const hunks = latest.current?.diff.hunks ?? [];
        const i = hunks.findIndex((h) => e.position.lineNumber >= h.newStart && e.position.lineNumber < h.newStart + Math.max(1, h.newLen));
        return i >= 0 ? i : c;
      });
    });
    return () => {
      sub.dispose();
      sub2.dispose();
      const m = d.getModel();
      d.dispose();
      m?.original.dispose();
      m?.modified.dispose();
    };
  }, []);

  const latest = useRef<DiffResult | null>(null);
  const load = async () => {
    try {
      const r = isCommit
        ? await invoke<DiffResult>("git_commit_diff", { root, hash, path })
        : await invoke<DiffResult>("git_diff", { root, path, staged: !!staged });
      latest.current = r;
      setData(r);
      setErr(null);
      const d = de.current!;
      const old = d.getModel();
      const lang = languageFor(path);
      const view = d.getModifiedEditor().saveViewState();
      d.setModel({ original: monaco.editor.createModel(r.original, lang), modified: monaco.editor.createModel(r.modified, lang) });
      if (view) d.getModifiedEditor().restoreViewState(view);
      old?.original.dispose();
      old?.modified.dispose();
      setCur((c) => Math.min(c, Math.max(0, r.diff.hunks.length - 1)));
    } catch (e) {
      setErr((e as { message?: string }).message ?? String(e));
    }
  };

  useEffect(() => {
    void load();
    if (isCommit) return;
    let un: (() => void) | undefined;
    void backend().then((b) => b.listen<string>("git-changed", (r) => { if (r === root) void load(); }).then((u) => (un = u)));
    return () => un?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [root, path, staged, hash]);

  const hunks = data?.diff.hunks ?? [];
  const reveal = (h: Hunk) => {
    const m = de.current!.getModifiedEditor();
    m.revealLineInCenter(Math.max(1, h.newStart));
    m.setPosition({ lineNumber: Math.max(1, h.newStart), column: 1 });
  };
  const hunkAction = async (action: "stage" | "unstage" | "discard", select: [number, number] | null) => {
    const run = () => act(root, action, () => invoke("git_hunk", { root, path, staged: !!staged, hunk: cur, select, action }));
    if (action === "discard") {
      confirmDialog({ title: "Discard this change?", body: <>Throws away {select ? `lines ${select[0]}–${select[1]}` : "this hunk"} in <b>{path}</b>.</>, confirmLabel: "Discard", danger: true, onConfirm: () => void run().then(load) });
      return;
    }
    await run();
    await load();
  };
  const fileAction = async (action: "stage" | "unstage") => {
    await act(root, action, () => invoke(action === "stage" ? "git_stage" : "git_unstage", { root, paths: [path] }));
    // Follow the file to the other side.
    useEditor.getState().open({ kind: "diff", projectId, root, path, staged: action === "stage" });
  };

  return (
    <div className="diff-view" data-testid="diff-view">
      <div className="diff-bar">
        <b>{path}</b>
        <span className="muted small">{isCommit ? `${hash!.slice(0, 7)}^ ↔ ${hash!.slice(0, 7)}` : staged ? "HEAD ↔ staged" : "staged ↔ working tree"}</span>
        <span className="spacer" />
        {!isCommit && (
          staged ? (
            <button className="btn small" onClick={() => void fileAction("unstage")}>Unstage file</button>
          ) : (
            <button className="btn small" onClick={() => void fileAction("stage")}>Stage file</button>
          )
        )}
        {!isCommit && !staged && <button className="btn small" onClick={() => useEditor.getState().open({ kind: "file", projectId, path: `${root}/${path}` })}>Open file</button>}
      </div>
      {!isCommit && hunks.length > 0 && (
        <div className="hunk-bar">
          <button className="icon-btn" onClick={() => { const i = Math.max(0, cur - 1); setCur(i); reveal(hunks[i]); }} aria-label="Previous change">‹</button>
          <span className="muted small">Change {cur + 1}/{hunks.length} <code>{hunks[cur]?.header.split("@@")[1]?.trim()}</code></span>
          <button className="icon-btn" onClick={() => { const i = Math.min(hunks.length - 1, cur + 1); setCur(i); reveal(hunks[i]); }} aria-label="Next change">›</button>
          <span className="spacer" />
          {staged ? (
            <>
              <button className="btn small" onClick={() => void hunkAction("unstage", null)} data-testid="unstage-hunk">Unstage hunk</button>
              {selLines && <button className="btn small" onClick={() => void hunkAction("unstage", selLines)}>Unstage lines {selLines[0]}–{selLines[1]}</button>}
            </>
          ) : (
            <>
              <button className="btn small primary" onClick={() => void hunkAction("stage", null)} data-testid="stage-hunk">Stage hunk</button>
              {selLines && <button className="btn small" onClick={() => void hunkAction("stage", selLines)} data-testid="stage-lines">Stage lines {selLines[0]}–{selLines[1]}</button>}
              <button className="btn small danger" onClick={() => void hunkAction("discard", selLines)}>{selLines ? "Discard lines" : "Discard hunk"}</button>
            </>
          )}
        </div>
      )}
      {err && <div className="git-error">{err}</div>}
      {data?.binary && <div className="placeholder"><p className="muted">Binary file — no text diff.</p></div>}
      <div className="monaco-host" ref={host} />
    </div>
  );
}

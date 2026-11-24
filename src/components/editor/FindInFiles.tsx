import { useEffect, useRef, useState } from "react";
import { invoke } from "../../ipc";
import { useEditor } from "../../store/editor";
import { useFleet } from "../../store/store";
import { confirmDialog } from "../Dialogs";
import { monaco } from "../../editor/monaco";
import type { Project } from "../../types";

interface Match { line: number; col: number; len: number; text: string }
interface FileMatches { path: string; matches: Match[] }

export function FindInFiles({ project }: { project: Project }) {
  const [q, setQ] = useState("");
  const [replace, setReplace] = useState("");
  const [regex, setRegex] = useState(false);
  const [cs, setCs] = useState(false);
  const [glob, setGlob] = useState("");
  const [res, setRes] = useState<FileMatches[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
    const f = () => input.current?.select();
    window.addEventListener("fleet:find-in-files", f);
    return () => window.removeEventListener("fleet:find-in-files", f);
  }, []);

  const search = async () => {
    if (!q) return setRes([]);
    setBusy(true);
    setErr(null);
    try {
      setRes(await invoke<FileMatches[]>("search_files", { root: project.rootPath, query: q, regex, caseSensitive: cs, glob }));
    } catch (e) {
      setErr(String(e));
    }
    setBusy(false);
  };
  useEffect(() => {
    const t = window.setTimeout(() => void search(), 250);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q, regex, cs, glob]);

  const openAt = (path: string, m: Match) => {
    useEditor.getState().open({ kind: "file", projectId: project.id, path: `${project.rootPath}/${path}` });
    // Reveal after the editor has loaded the model.
    const t0 = Date.now();
    const go = () => {
      const ed = monaco.editor.getEditors().find((e) => e.getModel()?.uri.fsPath === `${project.rootPath}/${path}`);
      if (ed) {
        ed.setSelection(new monaco.Range(m.line, m.col, m.line, m.col + m.len));
        ed.revealLineInCenter(m.line);
        ed.focus();
      } else if (Date.now() - t0 < 2000) setTimeout(go, 50);
    };
    go();
  };

  const total = res.reduce((n, f) => n + f.matches.length, 0);
  const replaceAll = () =>
    confirmDialog({
      title: `Replace ${total} match${total === 1 ? "" : "es"} in ${res.length} file${res.length === 1 ? "" : "s"}?`,
      body: <>“{q}” → “{replace}”. Files open in the editor reload from disk.</>,
      confirmLabel: "Replace all",
      danger: true,
      onConfirm: async () => {
        try {
          const n = await invoke<number>("replace_all", { root: project.rootPath, paths: res.map((r) => r.path), query: q, replacement: replace, regex, caseSensitive: cs });
          useFleet.getState().showToast(`Replaced ${n} match${n === 1 ? "" : "es"}`);
          void search();
        } catch (e) {
          useFleet.getState().showToast(String(e), "error");
        }
      },
    });

  return (
    <div className="find-files" data-testid="find-in-files">
      <div className="ff-inputs">
        <div className="ff-row">
          <input ref={input} className="field" placeholder="Search" value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Enter") void search(); }} data-testid="ff-query" />
          <button className={`toggle ${cs ? "on" : ""}`} title="Match case" onClick={() => setCs(!cs)}>Aa</button>
          <button className={`toggle ${regex ? "on" : ""}`} title="Regular expression" onClick={() => setRegex(!regex)}>.*</button>
        </div>
        <div className="ff-row">
          <input className="field" placeholder="Replace" value={replace} onChange={(e) => setReplace(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
          <button className="btn small" disabled={!total} onClick={replaceAll}>All</button>
        </div>
        <input className="field" placeholder="Files to include, e.g. src/**/*.ts" value={glob} onChange={(e) => setGlob(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
      </div>
      <div className="muted small pad-x">{busy ? "Searching…" : q ? `${total} result${total === 1 ? "" : "s"} in ${res.length} file${res.length === 1 ? "" : "s"}` : ""}</div>
      {err && <div className="git-error">{err}</div>}
      <div className="ff-results">
        {res.map((f) => (
          <div key={f.path}>
            <div className="ff-file" title={f.path}>{f.path.split("/").pop()} <span className="muted small">{f.path}</span> <span className="muted small">{f.matches.length}</span></div>
            {f.matches.map((m, i) => (
              <button key={i} className="ff-match" onClick={() => openAt(f.path, m)}>
                <span className="muted small">{m.line}</span>{" "}
                <span>{m.text.slice(Math.max(0, m.col - 30), m.col - 1)}<b className="hit">{m.text.slice(m.col - 1, m.col - 1 + m.len)}</b>{m.text.slice(m.col - 1 + m.len, m.col + 80)}</span>
              </button>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

import { useEffect, useMemo, useState } from "react";
import { create } from "zustand";
import { invoke } from "../../ipc";
import { useFleet } from "../../store/store";
import { useEditor } from "../../store/editor";
import { fuzzyFilter, Highlight } from "../../editor/fuzzy";

export const useQuickOpen = create<{ open: boolean }>(() => ({ open: false }));
const cache: Record<string, string[]> = {};

export function editorProject() {
  const st = useFleet.getState();
  const pid = st.editorProjectId ?? (st.activeSessionId ? st.sessions[st.activeSessionId]?.projectId : null) ?? st.projects[0]?.id;
  return st.projects.find((p) => p.id === pid) ?? null;
}

export function QuickOpen() {
  const open = useQuickOpen((s) => s.open);
  const [q, setQ] = useState("");
  const [files, setFiles] = useState<string[]>([]);
  const [i, setI] = useState(0);
  const project = open ? editorProject() : null;

  useEffect(() => {
    if (!project) return;
    setQ("");
    setI(0);
    setFiles(cache[project.rootPath] ?? []);
    void invoke<string[]>("quick_open_files", { root: project.rootPath }).then((f) => { cache[project.rootPath] = f; setFiles(f); }).catch(() => {});
  }, [open, project?.rootPath]); // eslint-disable-line react-hooks/exhaustive-deps

  const results = useMemo(() => fuzzyFilter(files, q, (f) => f, 50), [files, q]);
  if (!open || !project) return null;
  const close = () => useQuickOpen.setState({ open: false });
  const pick = (rel: string) => {
    close();
    useEditor.getState().open({ kind: "file", projectId: project.id, path: `${project.rootPath}/${rel}` });
  };
  return (
    <div className="overlay" onMouseDown={close}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label="Quick open">
        <input
          autoFocus
          className="palette-input"
          placeholder={`Open a file in ${project.name}…`}
          value={q}
          onChange={(e) => { setQ(e.target.value); setI(0); }}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Escape") close();
            if (e.key === "ArrowDown") { e.preventDefault(); setI((x) => Math.min(results.length - 1, x + 1)); }
            if (e.key === "ArrowUp") { e.preventDefault(); setI((x) => Math.max(0, x - 1)); }
            if (e.key === "Enter" && results[i]) pick(results[i].item);
          }}
          data-testid="quick-open-input"
        />
        <div className="palette-list">
          {results.map((r, j) => (
            <button key={r.item} className={`palette-item ${j === i ? "on" : ""}`} onMouseEnter={() => setI(j)} onClick={() => pick(r.item)}>
              <span className="pi-main"><Highlight text={r.item} hits={r.hits} /></span>
            </button>
          ))}
          {files.length > 0 && results.length === 0 && <div className="muted pad">No matching files.</div>}
        </div>
      </div>
    </div>
  );
}

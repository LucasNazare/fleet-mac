import { useEffect, useMemo, useState } from "react";
import { useFleet, orderedSessions } from "../store/store";
import { useGit, currentRoot } from "../store/git";
import { useEditor } from "../store/editor";
import { COMMANDS, keysFor, prettyKeys } from "../commands";
import { fuzzyFilter, Highlight } from "../editor/fuzzy";
import { invoke } from "../ipc";
import { STATE } from "../theme";
import { procKey } from "../types";
import { editorProject } from "./editor/QuickOpen";
import { guardWorking } from "./git/GitPanel";

interface Item {
  key: string;
  kind: "Session" | "Process" | "File" | "Branch" | "Command";
  label: string;
  detail?: string;
  keys?: string;
  run: () => void;
}

const fileCache: Record<string, string[]> = {};

/** ⌘K: jump to any session, process, file, branch or command. */
export function Palette() {
  const { projects, sessions, runs } = useFleet();
  const [q, setQ] = useState("");
  const [i, setI] = useState(0);
  const [files, setFiles] = useState<string[]>([]);
  const project = editorProject();
  const cur = currentRoot();
  const branches = useGit((g) => (cur ? g.repos[cur.root]?.branches : null));
  const close = () => useFleet.setState({ paletteOpen: false });

  useEffect(() => {
    if (cur) void useGit.getState().refresh(cur.root, ["branches"]);
    if (!project) return;
    setFiles(fileCache[project.rootPath] ?? []);
    void invoke<string[]>("quick_open_files", { root: project.rootPath }).then((f) => { fileCache[project.rootPath] = f; setFiles(f); }).catch(() => {});
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const items = useMemo<Item[]>(() => {
    const out: Item[] = [];
    for (const s of orderedSessions(projects, sessions)) {
      const p = projects.find((x) => x.id === s.projectId);
      out.push({ key: `s:${s.id}`, kind: "Session", label: `${s.glyph} · ${s.title}`, detail: `${p?.name ?? ""} · ${STATE[s.state].symbol} ${STATE[s.state].word}`, run: () => useFleet.getState().select(s.id) });
    }
    for (const p of projects) for (const d of p.processes) {
      const r = runs[procKey(p.id, d.id)];
      out.push({ key: `p:${p.id}:${d.id}`, kind: "Process", label: d.name || d.command, detail: `${p.name}${r?.status === "running" ? " · running" : ""}`, run: () => useFleet.getState().openProcess(procKey(p.id, d.id)) });
    }
    for (const b of branches ?? []) {
      if (b.remote || b.head || !cur) continue;
      out.push({ key: `b:${b.name}`, kind: "Branch", label: b.name, detail: "checkout", run: () => guardWorking(cur.root, "Checkout", () => void useGit.getState().act(cur.root, "checkout", () => invoke("git_run", { root: cur.root, args: ["checkout", b.name] }))) });
    }
    for (const c of COMMANDS) if (!c.hidden && c.id !== "palette") out.push({ key: `c:${c.id}`, kind: "Command", label: c.title, keys: prettyKeys(keysFor(c.id)), run: () => void c.run() });
    return out;
  }, [projects, sessions, runs, branches, cur]);

  const results = useMemo(() => {
    const base = fuzzyFilter(items, q, (x) => `${x.label} ${x.detail ?? ""}`, 40);
    if (q.length >= 2 && project) {
      const f = fuzzyFilter(files, q, (x) => x, 15).map((r) => ({
        item: { key: `f:${r.item}`, kind: "File" as const, label: r.item, detail: undefined, keys: undefined, run: () => useEditor.getState().open({ kind: "file", projectId: project.id, path: `${project.rootPath}/${r.item}` }) },
        score: r.score - 2,
        hits: r.hits,
      })) as { item: Item; score: number; hits: number[] }[];
      return [...base, ...f].sort((a, b) => b.score - a.score).slice(0, 50);
    }
    return base;
  }, [items, files, q, project]);

  return (
    <div className="overlay" onMouseDown={close}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label="Command palette">
        <input
          autoFocus
          className="palette-input"
          placeholder="Jump to a session, process, file, branch or command…"
          value={q}
          onChange={(e) => { setQ(e.target.value); setI(0); }}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Escape") close();
            if (e.key === "ArrowDown") { e.preventDefault(); setI((x) => Math.min(results.length - 1, x + 1)); }
            if (e.key === "ArrowUp") { e.preventDefault(); setI((x) => Math.max(0, x - 1)); }
            if (e.key === "Enter" && results[i]) { close(); results[i].item.run(); }
          }}
          data-testid="palette-input"
        />
        <div className="palette-list">
          {results.map((r, j) => (
            <button key={r.item.key} className={`palette-item ${j === i ? "on" : ""}`} onMouseEnter={() => setI(j)} onClick={() => { close(); r.item.run(); }} data-testid="palette-item">
              <span className={`pi-kind k-${r.item.kind}`}>{r.item.kind}</span>
              <span className="pi-main">{r.item.kind === "File" || !q ? r.item.label : <Highlight text={r.item.label} hits={r.hits.filter((h) => h < r.item.label.length)} />}</span>
              {r.item.detail && <span className="muted small">{r.item.detail}</span>}
              {r.item.keys && <kbd>{r.item.keys}</kbd>}
            </button>
          ))}
          {results.length === 0 && <div className="muted pad">Nothing matches.</div>}
        </div>
      </div>
    </div>
  );
}

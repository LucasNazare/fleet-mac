import { useMemo, useState } from "react";
import { useGit } from "../../store/git";
import { useEditor } from "../../store/editor";
import { useFleet } from "../../store/store";
import { invoke } from "../../ipc";
import type { Commit } from "../../types";
import { ContextMenu } from "../ContextMenu";
import { promptDialog, confirmDialog } from "../Dialogs";
import { guardWorking } from "./GitPanel";
import { LETTER_COLOR, relTime } from "./util";

const LANE_COLORS = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => `var(--lane-${n})`);
const W = 12;
const H = 26;

interface Row {
  lane: number;
  /** Lanes active through this row (top half) and after it (bottom half). */
  before: (string | null)[];
  after: (string | null)[];
  /** Lanes this commit's parents continue in. */
  parentLanes: number[];
}

/** Simple lane assignment for the history graph. */
export function computeLanes(commits: Commit[]): Row[] {
  let lanes: (string | null)[] = [];
  return commits.map((c) => {
    const before = lanes.slice();
    let lane = lanes.indexOf(c.hash);
    if (lane < 0) {
      lane = lanes.indexOf(null);
      if (lane < 0) lane = lanes.length;
      lanes[lane] = c.hash;
    }
    // Other lanes that also pointed at this commit merge into it.
    lanes = lanes.map((h, i) => (h === c.hash && i !== lane ? null : h));
    const parentLanes: number[] = [];
    c.parents.forEach((p, i) => {
      if (i === 0) {
        lanes[lane] = p;
        parentLanes.push(lane);
      } else {
        let l = lanes.indexOf(p);
        if (l < 0) {
          l = lanes.indexOf(null);
          if (l < 0) l = lanes.length;
          lanes[l] = p;
        }
        parentLanes.push(l);
      }
    });
    if (c.parents.length === 0) lanes[lane] = null;
    while (lanes.length && lanes[lanes.length - 1] === null) lanes.pop();
    return { lane, before: before.length > lane ? before : [...before, ...Array(lane + 1 - before.length).fill(null)], after: lanes.slice(), parentLanes };
  });
}

function Graph({ row, width }: { row: Row; width: number }) {
  const x = (l: number) => l * W + W / 2;
  const color = (l: number) => LANE_COLORS[l % LANE_COLORS.length];
  return (
    <svg width={width} height={H} className="lane-svg" aria-hidden>
      {row.before.map((h, l) => h && l !== row.lane ? <line key={`b${l}`} x1={x(l)} y1={0} x2={x(l)} y2={H / 2} style={{ stroke: color(l) }} strokeWidth={1.5} /> : null)}
      {row.before[row.lane] && <line x1={x(row.lane)} y1={0} x2={x(row.lane)} y2={H / 2} style={{ stroke: color(row.lane) }} strokeWidth={1.5} />}
      {row.after.map((h, l) => h && !row.parentLanes.includes(l) ? <line key={`a${l}`} x1={x(l)} y1={H / 2} x2={x(l)} y2={H} style={{ stroke: color(l) }} strokeWidth={1.5} /> : null)}
      {row.parentLanes.map((pl, i) => (
        <path key={`p${i}`} d={`M ${x(row.lane)} ${H / 2} C ${x(row.lane)} ${H * 0.85}, ${x(pl)} ${H * 0.75}, ${x(pl)} ${H}`} style={{ stroke: color(pl) }} strokeWidth={1.5} fill="none" />
      ))}
      <circle cx={x(row.lane)} cy={H / 2} r={3.5} style={{ fill: color(row.lane), stroke: "var(--panel)" }} strokeWidth={1.5} />
    </svg>
  );
}

export function HistoryTab({ root }: { root: string }) {
  const { repos, act } = useGit();
  const repo = repos[root];
  const log = repo?.log;
  const rows = useMemo(() => (log ? computeLanes(log) : []), [log]);
  const width = Math.min(8, Math.max(1, ...rows.map((r) => Math.max(r.before.length, r.after.length, r.lane + 1)))) * W;
  const [open, setOpen] = useState<string | null>(null);
  const [files, setFiles] = useState<{ status: string; path: string }[]>([]);
  const [menu, setMenu] = useState<{ x: number; y: number; c: Commit } | null>(null);
  const base = repo?.status?.top ?? root;
  if (!log) return <div className="muted pad">Loading history…</div>;
  if (!log.length) return <div className="muted pad">No commits yet.</div>;

  const toggle = async (c: Commit) => {
    if (open === c.hash) return setOpen(null);
    setOpen(c.hash);
    setFiles(await invoke<{ status: string; path: string }[]>("git_commit_files", { root, hash: c.hash }).catch(() => []));
  };
  const run = (label: string, args: string[]) => void act(root, label, () => invoke("git_run", { root, args }));

  return (
    <div className="history" data-testid="history">
      {log.map((c, i) => (
        <div key={c.hash}>
          <div className={`commit-row ${open === c.hash ? "open" : ""}`} onClick={() => void toggle(c)} onContextMenu={(e) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY, c }); }} title={`${c.hash}\n${c.author} · ${new Date(c.time * 1000).toLocaleString()}`}>
            <Graph row={rows[i]} width={width} />
            <span className="commit-text">
              <span className="subject">{c.subject}</span>
              {c.refs && <span className="refs">{c.refs.split(", ").filter((r) => !r.startsWith("tag:") || true).slice(0, 3).map((r) => <span key={r} className="ref">{r.replace("HEAD -> ", "HEAD→")}</span>)}</span>}
              <span className="muted small commit-meta">{c.hash.slice(0, 7)} · {c.author} · {relTime(c.time)}</span>
            </span>
          </div>
          {open === c.hash && (
            <div className="commit-files">
              {files.map((f) => (
                <button key={f.path} className="git-row small" onClick={() => useEditor.getState().open({ kind: "commit", projectId: useFleet.getState().projects.find((p) => base.startsWith(p.rootPath))?.id ?? "", root: base, hash: c.hash, path: f.path })}>
                  <span className="letter" style={{ color: LETTER_COLOR[f.status] ?? "var(--muted)" }}>{f.status}</span>
                  <span className="gpath">{f.path}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      ))}
      {menu && (
        <ContextMenu x={menu.x} y={menu.y} onClose={() => setMenu(null)} items={[
          { label: "Copy hash", onClick: () => void navigator.clipboard.writeText(menu.c.hash) },
          { label: "Checkout (detached)", onClick: () => guardWorking(root, "Checkout", () => run("checkout", ["checkout", menu.c.hash])) },
          { label: "Create branch here…", onClick: () => promptDialog({ title: "New branch", label: `From ${menu.c.hash.slice(0, 7)}`, placeholder: "feature/name", confirmLabel: "Create and checkout", onSubmit: (n) => run("branch", ["checkout", "-b", n, menu.c.hash]) }) },
          { label: "Cherry-pick onto current branch", onClick: () => run("cherry-pick", ["cherry-pick", menu.c.hash]) },
          { label: "Revert…", onClick: () => confirmDialog({ title: "Revert this commit?", body: <>Creates a new commit undoing “{menu.c.subject}”.</>, confirmLabel: "Revert", onConfirm: () => run("revert", ["revert", "--no-edit", menu.c.hash]) }) },
        ]} />
      )}
    </div>
  );
}

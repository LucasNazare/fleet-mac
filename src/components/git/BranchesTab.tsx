import { useState } from "react";
import { useGit } from "../../store/git";
import { useFleet } from "../../store/store";
import { invoke } from "../../ipc";
import type { Branch } from "../../types";
import { ContextMenu } from "../ContextMenu";
import { confirmDialog, promptDialog } from "../Dialogs";
import { Chip } from "../common";
import { guardWorking } from "./GitPanel";
import { relTime } from "./util";

export function BranchesTab({ root }: { root: string }) {
  const { repos, act } = useGit();
  const { sessions, select } = useFleet();
  const repo = repos[root];
  const [menu, setMenu] = useState<{ x: number; y: number; b: Branch } | null>(null);
  const branches = repo?.branches;
  if (!branches) return <div className="muted pad">Loading branches…</div>;
  const current = branches.find((b) => b.head)?.name ?? repo?.status?.status?.branch ?? "";
  const run = (label: string, args: string[]) => act(root, label, () => invoke("git_run", { root, args }));
  const local = branches.filter((b) => !b.remote);
  const remote = branches.filter((b) => b.remote);
  const mainRoot = repo?.worktrees?.[0]?.path ?? root;

  const items = (b: Branch) => {
    const name = b.name;
    const checkout = () => guardWorking(root, "Checkout", () => void run("checkout", b.remote ? ["checkout", "--track", name] : ["checkout", name]));
    return [
      { label: "Checkout", disabled: b.head || (!!b.worktree && !b.head), onClick: checkout },
      { label: "New branch from here…", onClick: () => promptDialog({ title: "New branch", label: `From ${name}`, placeholder: "feature/name", confirmLabel: "Create and checkout", onSubmit: (n) => void run("branch", ["checkout", "-b", n, name]) }) },
      { label: `Merge into ${current}`, disabled: b.head, onClick: () => void run("merge", ["merge", "--no-edit", name]) },
      { label: `Rebase ${current} onto ${name}`, disabled: b.head, onClick: () => guardWorking(root, "Rebase", () => void run("rebase", ["rebase", name])) },
      ...(b.remote ? [] : [
        { label: "Publish (push -u)", disabled: !!b.upstream, onClick: () => void run("push", ["push", "-u", "origin", name]) },
        { label: "Delete…", danger: true, disabled: b.head, onClick: () => confirmDialog({
          title: `Delete branch ${name}?`, body: "Only if it's fully merged; otherwise git refuses (use force delete).", confirmLabel: "Delete", danger: true,
          extra: { label: "Force delete", onClick: () => void run("branch", ["branch", "-D", name]) },
          onConfirm: () => void run("branch", ["branch", "-d", name]),
        }) },
      ]),
    ];
  };

  const Row = ({ b }: { b: Branch }) => (
    <div className={`branch-row ${b.head ? "head" : ""}`} onContextMenu={(e) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY, b }); }}
      onDoubleClick={() => !b.head && guardWorking(root, "Checkout", () => void run("checkout", b.remote ? ["checkout", "--track", b.name] : ["checkout", b.name]))} title="Double-click to checkout · right-click for actions">
      <span className="bname">{b.head ? "● " : ""}{b.name}</span>
      {(b.ahead > 0 || b.behind > 0) && <span className="muted small">↑{b.ahead} ↓{b.behind}</span>}
      {b.gone && <span className="muted small">upstream gone</span>}
      {b.worktree && !b.head && <span className="tag">worktree</span>}
      <span className="spacer" />
      <span className="muted small">{relTime(b.time)}</span>
      <button className="icon-btn" aria-label={`Actions for ${b.name}`} onClick={(e) => setMenu({ x: e.clientX, y: e.clientY, b })}>⋯</button>
    </div>
  );

  const wts = (repo?.worktrees ?? []).slice(1);
  return (
    <div className="branches">
      <div className="git-group"><span>Local</span><span className="muted">{local.length}</span><span className="spacer" />
        <button className="link" onClick={() => promptDialog({ title: "New branch", label: `From ${current}`, placeholder: "feature/name", confirmLabel: "Create and checkout", onSubmit: (n) => void run("branch", ["checkout", "-b", n]) })}>+ new branch</button>
      </div>
      {local.map((b) => <Row key={b.full} b={b} />)}
      {remote.length > 0 && <div className="git-group"><span>Remote</span><span className="muted">{remote.length}</span></div>}
      {remote.map((b) => <Row key={b.full} b={b} />)}
      <div className="git-group"><span>Worktrees</span><span className="muted">{wts.length}</span></div>
      {wts.length === 0 && <div className="muted pad small">Sessions started with “Run in its own git worktree” appear here.</div>}
      {wts.map((w) => {
        const s = Object.values(sessions).find((x) => x.worktreePath === w.path && !x.archived);
        return (
          <div key={w.path} className="wt-row">
            {s ? <Chip s={s} size={18} /> : <span className="chip-empty" />}
            <span className="gpath" title={w.path}>{w.branch ?? w.head.slice(0, 7)}<span className="muted small"> · {w.path.split("/").slice(-2).join("/")}</span></span>
            <span className="wt-actions">
              {s && <button className="link" onClick={() => select(s.id)}>Open session</button>}
              {w.branch && <button className="link" onClick={() => guardWorking(mainRoot, "Merge", () => void act(root, "merge", () => invoke("git_run", { root: mainRoot, args: ["merge", "--no-edit", w.branch!] })))}>Merge into {repo?.worktrees?.[0]?.branch ?? "main"}</button>}
              <button className="link danger" onClick={() => confirmDialog({ title: "Remove worktree?", body: <>Deletes the folder {w.path}. Its branch {w.branch} is kept.</>, confirmLabel: "Remove", danger: true, onConfirm: () => void run("worktree", ["worktree", "remove", w.path]) })}>Remove</button>
            </span>
          </div>
        );
      })}
      {menu && <ContextMenu x={menu.x} y={menu.y} onClose={() => setMenu(null)} items={items(menu.b)} />}
    </div>
  );
}

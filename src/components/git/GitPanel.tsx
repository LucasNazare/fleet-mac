import { memo, useEffect, useRef, useState } from "react";
import { useFleet } from "../../store/store";
import { currentRoot, useGit, workingSessionsIn } from "../../store/git";
import { invoke } from "../../ipc";
import { ChangesTab } from "./ChangesTab";
import { HistoryTab } from "./HistoryTab";
import { BranchesTab } from "./BranchesTab";
import { StashTab } from "./StashTab";
import { confirmDialog } from "../Dialogs";
import { relTime } from "./util";
import { GitSyncHeader } from "../SyncUi";
import { useSync } from "../../store/sync";

/** Ask first when sessions are working in this checkout (their files would change under them). */
export function guardWorking(root: string, what: string, run: () => void) {
  const busy = workingSessionsIn(root);
  if (!busy.length) return run();
  confirmDialog({
    title: `${what} while agents are working?`,
    body: <>{busy.map((s) => `${s.glyph} · ${s.title}`).join(", ")} {busy.length === 1 ? "is" : "are"} working in this checkout. Their files would change under them.</>,
    confirmLabel: `${what} anyway`,
    danger: true,
    onConfirm: run,
  });
}

export function remote(root: string, op: string, label: string) {
  const g = useGit.getState();
  const go = () => void g.act(root, label, () => invoke("git_remote", { root, op }));
  if (op === "pull" || op === "pull_rebase") guardWorking(root, "Pull", go);
  else go();
}

export const GitPanel = memo(function GitPanel() {
  // Select a primitive: a fresh object per call would re-render forever.
  const root = useFleet(() => currentRoot()?.root ?? null);
  const remoteName = useFleet((s) => {
    const a = s.activeSessionId ? s.sessions[s.activeSessionId] : null;
    return a?.remote ? s.projects.flatMap((p) => p.sshConnections ?? []).find((c) => c.id === a.remote)?.name ?? "the server" : null;
  });
  const { repos, tab, set, output, outputOpen, autoOnly, ensure, refresh } = useGit();
  const projectId = useFleet(() => currentRoot()?.projectId ?? null);
  const activeSession = useFleet((s) => (s.activeSessionId ? s.sessions[s.activeSessionId] ?? null : null));
  const syncCfg = useSync((s) => (projectId ? s.configs[projectId] : undefined));
  const repo = root ? repos[root] : undefined;
  const panelRef = useRef<HTMLDivElement>(null);
  const [branchMenu, setBranchMenu] = useState(false);

  useEffect(() => {
    if (root) ensure(root);
  }, [root, ensure]);
  useEffect(() => {
    if (!root) return;
    if (tab === "history") void refresh(root, ["log"]);
    if (tab === "branches") void refresh(root, ["branches"]);
    if (tab === "stash") void refresh(root, ["stashes"]);
  }, [tab, root, refresh]);
  useEffect(() => {
    const focus = () => panelRef.current?.focus();
    window.addEventListener("fleet:focus-git", focus);
    return () => window.removeEventListener("fleet:focus-git", focus);
  }, []);

  if (!root) return <aside className="right-panel"><div className="panel-title">Git</div><p className="muted pad">Add a project to see its git status.</p></aside>;
  if (remoteName) {
    return (
      <aside className="right-panel" data-testid="git-panel">
        <div className="panel-title">Git</div>
        <p className="muted pad" data-testid="remote-notice">This session works on <b>{remoteName}</b> over SSH. The git panel and editor show local checkouts only for now — ask Claude, or use ⌃N for a shell on the server.</p>
      </aside>
    );
  }
  const st = repo?.status;
  if (st && !st.isRepo) {
    return (
      <aside className="right-panel">
        <div className="panel-title">Git</div>
        <div className="pad">
          <p className="muted">{root} isn't a git repository.</p>
          <button className="btn" onClick={() => void useGit.getState().act(root, "init", () => invoke("git_run", { root, args: ["init"] }))}>git init</button>
        </div>
      </aside>
    );
  }
  const s = st?.status;
  const busy = repo?.busy;
  const myOutput = autoOnly ? output.filter((o) => o.auto) : output.filter((o) => o.root === root || o.root === st?.top);
  const mainLocked = !!syncCfg?.enabled && !!s && [syncCfg.defaultBranch, "main", "master"].includes(s.branch);
  const localBranches = (repo?.branches ?? []).filter((b) => !b.remote);

  return (
    <aside className="right-panel git" tabIndex={-1} ref={panelRef} data-testid="git-panel">
      <div className="git-head">
        <div className="branch-wrap">
          <button className="branch-btn" onClick={() => { setBranchMenu((v) => !v); void refresh(root, ["branches"]); }} title="Switch branch" data-testid="branch-btn">
            ⎇ {s?.branch ?? "…"} ▾
          </button>
          {branchMenu && (
            <div className="menu branch-menu" onMouseLeave={() => setBranchMenu(false)}>
              {localBranches.map((b) => (
                <button key={b.full} className="menu-item" disabled={b.head} onClick={() => {
                  setBranchMenu(false);
                  guardWorking(root, "Checkout", () => void useGit.getState().act(root, "checkout", () => invoke("git_run", { root, args: ["checkout", b.name] })));
                }}>
                  {b.head ? "● " : ""}{b.name}{b.worktree && !b.head ? <span className="muted"> · worktree</span> : null}
                </button>
              ))}
              <button className="menu-item" onClick={() => { setBranchMenu(false); set({ tab: "branches" }); }}>All branches…</button>
            </div>
          )}
        </div>
        {s && (
          <span className="ab muted" title={s.upstream ? `Tracking ${s.upstream}` : "No upstream"}>
            ↑{s.ahead} ↓{s.behind}
          </span>
        )}
        <span className="spacer" />
        {busy && ["fetch", "pull", "push"].includes(busy) ? (
          <>
            <span className="spinner" aria-label={`${busy} running`} /> <span className="muted small">{busy}…</span>
            <button className="btn small" onClick={() => void invoke("git_cancel", { root })}>Cancel</button>
          </>
        ) : (
          <>
            <button className="btn small" onClick={() => remote(root, "fetch", "fetch")} title="git fetch --all --prune">Fetch</button>
            <button className="btn small" onClick={() => remote(root, "pull", "pull")}>Pull</button>
            <button className="btn small" disabled={mainLocked} title={mainLocked ? `${s?.branch} changes only through Ship (⌘⇧D)` : undefined} onClick={() => remote(root, s?.upstream ? "push" : "set_upstream_push", "push")} data-testid="push-btn">Push</button>
          </>
        )}
      </div>
      {projectId && <GitSyncHeader projectId={projectId} session={activeSession?.projectId === projectId ? activeSession : null} />}
      {st?.merging && <div className="merge-bar">Merge/rebase in progress — resolve conflicts, stage, then commit.</div>}
      {repo?.error && (
        <div className="git-error" role="alert" data-testid="git-error">
          <div>✕ {repo.error.message}</div>
          <div className="err-actions">
            {repo.error.fix && (
              <button className="btn small primary" onClick={() => {
                const fix = repo.error!.fix!;
                if (fix === "stash") void useGit.getState().act(root, "stash", () => invoke("git_run", { root, args: ["stash", "push", "-u"] }));
                else if (fix === "init") void useGit.getState().act(root, "init", () => invoke("git_run", { root, args: ["init"] }));
                else remote(root, fix, fix.includes("push") ? "push" : "pull");
              }}>{repo.error.fixLabel}</button>
            )}
            <button className="btn small" onClick={() => set({ outputOpen: true })}>Details</button>
            <button className="btn small" onClick={() => useGit.setState((g) => ({ repos: { ...g.repos, [root]: { ...g.repos[root], error: null } } }))}>Dismiss</button>
          </div>
        </div>
      )}
      <nav className="git-tabs">
        {(["changes", "history", "branches", "stash"] as const).map((t) => (
          <button key={t} className={`tab ${tab === t ? "on" : ""}`} onClick={() => set({ tab: t })}>
            {t === "changes" ? `Changes${s ? ` ${s.files.length}` : ""}` : t[0].toUpperCase() + t.slice(1)}
          </button>
        ))}
      </nav>
      <div className="git-body">
        {tab === "changes" && <ChangesTab root={root} />}
        {tab === "history" && <HistoryTab root={root} />}
        {tab === "branches" && <BranchesTab root={root} />}
        {tab === "stash" && <StashTab root={root} />}
      </div>
      <div className={`output-drawer ${outputOpen ? "open" : ""}`}>
        <button className="output-toggle" onClick={() => set({ outputOpen: !outputOpen })} data-testid="output-toggle">
          {outputOpen ? "▾" : "▸"} Output <span className="muted">({myOutput.length})</span>
        </button>
        {outputOpen && autoOnly && <button className="filter-pill" onClick={() => set({ autoOnly: false })} data-testid="auto-only">sync log ✕</button>}
        {outputOpen && (
          <div className="output-list" data-testid="output-list">
            {myOutput.length === 0 && <div className="muted pad">Every git command fleet runs shows up here.</div>}
            {myOutput.slice().reverse().map((o) => (
              <div key={o.id} className={`out-entry ${o.code === 0 ? "" : "fail"}`}>
                <div className="out-cmd">$ {o.cmd} <span className="muted small">· {o.ms} ms · {relTime(o.at / 1000)}{o.code !== 0 ? ` · exit ${o.code}` : ""}</span></div>
                {o.output && <pre>{o.output}</pre>}
              </div>
            ))}
          </div>
        )}
      </div>
    </aside>
  );
});

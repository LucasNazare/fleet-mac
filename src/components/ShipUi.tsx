// Stage B/C UI: sessions on other computers, Ship (⌘⇧D), Add project from GitHub, Actions cost guard.
import { useEffect, useMemo, useState } from "react";
import type { GhRepo, OtherSession, ShipInfo, ShippedCommit, WorkflowFinding } from "../types";
import { useFleet } from "../store/store";
import { useSync } from "../store/sync";
import { backend, invoke } from "../ipc";
import * as pool from "../terminal/termPool";
import { Chip } from "./common";
import { confirmDialog } from "./Dialogs";
import { relTime } from "./git/util";

const ago = (iso: string) => relTime(Date.parse(iso) / 1000);

// ---------------------------------------------------------------- On other computers

async function continueHere(o: OtherSession, take: boolean) {
  const { cols, rows } = pool.currentSize();
  try {
    const s = await invoke<{ id: string }>("sync_continue", { projectId: o.projectId, branch: o.branch, take, cols, rows });
    useFleet.getState().select(s.id);
    useFleet.getState().showToast(`Continuing ${o.glyph} · ${o.title} here`);
  } catch (e) {
    const msg = String(e);
    if (msg.includes("ACTIVE:")) {
      const who = msg.split("ACTIVE:")[1];
      confirmDialog({
        title: `${o.glyph} is open on another Mac`,
        body: <>{who}. Taking over moves it to this Mac; the other Mac stops pushing to it.</>,
        confirmLabel: "Take over",
        danger: true,
        onConfirm: () => void continueHere(o, true),
      });
    } else useFleet.getState().showToast(msg.replace(/^Error: /, ""), "error");
  }
}

/** "Fix with Claude": the branch's session (or a new one on it) resolves the conflict with main. */
export async function fixConflict(projectId: string, branch: string, why?: string | null) {
  const { cols, rows } = pool.currentSize();
  try {
    const s = await invoke<{ id: string; glyph: string; title: string }>("sync_fix_conflict", { projectId, branch, why: why ?? null, cols, rows });
    useFleet.getState().select(s.id);
    useFleet.getState().showToast(`${s.glyph} · ${s.title} is fixing the conflict`);
    return true;
  } catch (e) {
    useFleet.getState().showToast(String(e).replace(/^Error: /, ""), "error");
    return false;
  }
}

export function OtherComputers({ projectId }: { projectId: string }) {
  const all = useSync((s) => s.others);
  const list = useMemo(() => all.filter((o) => o.projectId === projectId), [all, projectId]);
  const [busy, setBusy] = useState<string | null>(null);
  if (list.length === 0) return null;
  return (
    <>
      <div className="subhead">On other computers</div>
      {list.map((o) => (
        <div key={o.branch} className="other-row" data-testid="other-session" title={o.recap.join("\n")}>
          <Chip s={o} />
          <div className="row-text">
            <div className="row-title">{o.title}</div>
            <div className="row-state muted">
              {o.activeOn ? <span style={{ color: "var(--amber)" }}>active on {o.activeOn.replace(/^Active on /, "")} · {o.activeAgo}</span> : <>{o.lastMachine} · {ago(o.updatedAt)}</>}
              {o.commits > 0 && <> · {o.commits} commit{o.commits === 1 ? "" : "s"}</>}
            </div>
          </div>
          <button
            className="btn small"
            disabled={busy === o.branch}
            data-testid="continue-here"
            onClick={async () => {
              setBusy(o.branch);
              await continueHere(o, false);
              setBusy(null);
            }}
          >
            {busy === o.branch ? "…" : o.activeOn ? "Take over" : "Continue here"}
          </button>
        </div>
      ))}
    </>
  );
}

// ---------------------------------------------------------------- Ship sheet

export function ShipSheet() {
  const projectId = useSync((s) => s.ship);
  const project = useFleet((s) => s.projects.find((p) => p.id === projectId));
  const run = useSync((s) => (projectId ? s.runs[projectId] : undefined));
  const [info, setInfo] = useState<ShipInfo | null>(null);
  const [order, setOrder] = useState<string[]>([]);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [tab, setTab] = useState<"ship" | "history">("ship");
  const [history, setHistory] = useState<ShippedCommit[]>([]);
  const [drag, setDrag] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = async () => {
    if (!projectId) return;
    setError(null);
    try {
      const i = await invoke<ShipInfo>("ship_info", { projectId });
      setInfo(i);
      setOrder(i.candidates.map((c) => c.branch));
      setPicked(new Set(i.candidates.filter((c) => c.ready).map((c) => c.branch)));
      setHistory(await invoke<ShippedCommit[]>("ship_history", { projectId }).catch(() => []));
    } catch (e) {
      setError(String(e));
    }
  };
  useEffect(() => {
    setInfo(null);
    setTab("ship");
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);
  useEffect(() => {
    if (run && !run.running && run.outcome) void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [run?.running]);
  if (!projectId || !project) return null;
  const close = () => useSync.setState({ ship: null });
  const byBranch = Object.fromEntries((info?.candidates ?? []).map((c) => [c.branch, c]));
  const chosen = order.filter((b) => picked.has(b));
  const start = async () => {
    useSync.setState((s) => ({ runs: { ...s.runs, [projectId]: { running: true, log: "", steps: [], outcome: null } } }));
    try {
      await invoke("ship_run", { projectId, branches: chosen });
    } catch (e) {
      useFleet.getState().showToast(String(e), "error");
      useSync.setState((s) => ({ runs: { ...s.runs, [projectId]: { ...s.runs[projectId], running: false } } }));
    }
  };
  const redeploy = (tag?: string) =>
    confirmDialog({
      title: tag ? `Redeploy ${tag}?` : "Redeploy the previous version?",
      body: <>Runs <code>{info?.deployCommand}</code> at {tag ?? "the previous deploy tag"}. main isn't changed.</>,
      confirmLabel: "Redeploy",
      danger: true,
      onConfirm: async () => {
        useSync.setState((s) => ({ runs: { ...s.runs, [projectId]: { running: true, log: "", steps: [], outcome: null } } }));
        try {
          const t = await invoke<string>("ship_redeploy", { projectId, tag: tag ?? null });
          useFleet.getState().showToast(`Redeployed ${t}`);
        } catch (e) {
          useFleet.getState().showToast(String(e), "error");
        }
        useSync.setState((s) => ({ runs: { ...s.runs, [projectId]: { ...s.runs[projectId], running: false } } }));
      },
    });
  const retryDeploy = async () => {
    useSync.setState((s) => ({ runs: { ...s.runs, [projectId]: { running: true, log: "", steps: [], outcome: null } } }));
    try {
      const t = await invoke<string>("ship_retry_deploy", { projectId });
      useFleet.getState().showToast(`Deployed ${t}`);
    } catch (e) {
      useFleet.getState().showToast(String(e), "error");
    }
    useSync.setState((s) => ({ runs: { ...s.runs, [projectId]: { ...s.runs[projectId], running: false } } }));
    void load();
  };
  const lastDeploy = info?.deploys[info.deploys.length - 1];
  const deployPending = !!info?.deployCommand && (lastDeploy?.result === "deploy_failed" || lastDeploy?.result === "deploy_skipped");
  const undo = (c: ShippedCommit) =>
    confirmDialog({
      title: `Undo “${c.subject}”?`,
      body: <>fleet makes a revert commit on a new branch. It goes live with the next Ship; history isn't rewritten.</>,
      confirmLabel: "Create undo",
      onConfirm: async () => {
        try {
          const b = await invoke<string>("ship_undo", { projectId, commit: c.commit });
          useFleet.getState().showToast(`Undo ready on ${b} — ship it when you're ready`);
          await load();
          setTab("ship");
        } catch (e) {
          useFleet.getState().showToast(String(e), "error");
        }
      },
    });
  const outcome = run?.outcome;
  const running = !!run?.running;
  const deployedCount = info?.deploys.filter((d) => d.result === "deployed").length ?? 0;
  return (
    <div className="overlay" onMouseDown={() => !running && close()}>
      <div className="sheet ship-sheet" role="dialog" aria-label="Ship" data-testid="ship-sheet" onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape" && !running) close(); }}>
        <div className="ship-head">
          <h3>Ship · {project.name}</h3>
          <nav className="ship-tabs">
            <button className={`tab ${tab === "ship" ? "on" : ""}`} onClick={() => setTab("ship")}>Ship</button>
            <button className={`tab ${tab === "history" ? "on" : ""}`} onClick={() => setTab("history")} data-testid="ship-history-tab">History</button>
          </nav>
        </div>
        {error && <div className="git-sync-errors">{error}</div>}
        {!info && !error && <p className="muted">Checking GitHub…</p>}
        {info && tab === "ship" && (
          <>
            <p className="muted small">
              Shipping merges each ticked branch into <b>{info.defaultBranch}</b> in this order ({info.mergeStyle === "squash" ? "one commit per feature" : info.mergeStyle}),
              {info.checks.length ? " runs the checks," : ""} pushes {info.defaultBranch}{info.protected ? " through a pull request" : ""}
              {info.deployCommand ? <> and deploys</> : null}. A branch that conflicts or fails a check is skipped and its session is told why.
            </p>
            <div className="ship-list" data-testid="ship-list">
              {order.length === 0 && <p className="muted pad">Nothing to ship: no branch is ahead of {info.defaultBranch}.</p>}
              {order.map((b) => {
                const c = byBranch[b];
                if (!c) return null;
                const res = outcome?.results.find(([x]) => x === b)?.[1];
                return (
                  <div
                    key={b}
                    className={`ship-row ${drag === b ? "dragging" : ""} ${c.ready ? "" : "not-ready"}`}
                    draggable={!running}
                    onDragStart={() => setDrag(b)}
                    onDragEnd={() => setDrag(null)}
                    onDragOver={(e) => {
                      e.preventDefault();
                      if (!drag || drag === b) return;
                      const o = order.filter((x) => x !== drag);
                      o.splice(o.indexOf(b), 0, drag);
                      setOrder(o);
                    }}
                    data-testid="ship-row"
                    data-branch={b}
                  >
                    <span className="drag-handle" title="Drag to set the order">⋮⋮</span>
                    <input type="checkbox" disabled={!c.ready || running} checked={picked.has(b)} onChange={(e) => {
                      const n = new Set(picked);
                      if (e.target.checked) n.add(b); else n.delete(b);
                      setPicked(n);
                    }} aria-label={`Ship ${c.title}`} />
                    <Chip s={{ glyph: c.glyph, color: c.color }} size={20} />
                    <div className="row-text">
                      <div className="row-title">{c.title}</div>
                      <div className="row-state muted">
                        {c.commits} commit{c.commits === 1 ? "" : "s"}{c.stat ? ` · ${c.stat.replace(/ changed|\(\+\)|\(-\)/g, "")}` : ""}{c.machine ? ` · ${c.machine}` : ""}
                        {c.reason && <span style={{ color: "var(--amber)" }}> · {c.reason}</span>}
                      </div>
                    </div>
                    {(res?.kind === "conflict" || c.reason === "Has a conflict") && !running && (
                      <button
                        className="btn small"
                        data-testid="fix-conflict"
                        title="Claude merges main into this branch, resolves the conflict and runs the checks; then ship it again"
                        onClick={async () => {
                          const why = res?.kind === "conflict" ? `Ship skipped this branch: it conflicts with ${info.defaultBranch}${res.files.length ? ` in ${res.files.join(", ")}` : ""}.` : null;
                          if (await fixConflict(projectId, b, why)) close();
                        }}
                      >
                        Fix with Claude
                      </button>
                    )}
                    {res && (
                      outcome && !outcome.pushed && (res.kind === "merged" || res.kind === "empty") ? (
                        <span className="ship-res" data-testid="ship-result" data-kind="notShipped" style={{ color: "var(--muted)" }}>not shipped</span>
                      ) : (
                        <span className="ship-res" data-testid="ship-result" data-kind={res.kind} style={{ color: res.kind === "merged" ? "var(--green)" : res.kind === "empty" ? "var(--muted)" : "var(--red)" }}>
                          {res.kind === "merged" ? "✓ shipped" : res.kind === "conflict" ? "⚠ conflict" : res.kind === "checksFailed" ? "✕ checks" : "nothing new"}
                        </span>
                      )
                    )}
                  </div>
                );
              })}
            </div>
            <div className="ship-cmds small">
              <div><span className="muted">Checks:</span> {info.checks.length ? info.checks.map((c) => <code key={c}>{c}</code>) : <span className="muted">none (add them in fleet.yml)</span>}</div>
              <div><span className="muted">Deploy:</span> {info.deployCommand ? <code>{info.deployCommand}</code> : <span className="muted">none, ship only (tag ship-…)</span>}</div>
            </div>
            {(running || run?.log) && (
              <pre className="ship-log" data-testid="ship-log">{(run?.log ?? "").replace(/\x1b\[[0-9;]*[A-Za-z]/g, "").slice(-20000) || "Starting…"}</pre>
            )}
            {outcome && !running && (
              <div className={`ship-summary ${outcome.error ? "bad" : ""}`} data-testid="ship-summary">
                {outcome.error ? `✕ ${outcome.error}${outcome.pushed ? "" : " Nothing was shipped; every branch is unchanged."}` : <>
                  ✓ {outcome.results.filter(([, r]) => r.kind === "merged").length} shipped
                  {outcome.tag && <> · tagged <code>{outcome.tag}</code></>}
                  {outcome.deployCode != null && (outcome.deployCode === 0 ? " · deployed" : ` · deploy failed (exit ${outcome.deployCode}) — main stays as pushed; fix forward or Redeploy previous`)}
                  {outcome.results.some(([, r]) => r.kind === "conflict" || r.kind === "checksFailed") && " · skipped sessions were told why"}
                </>}
              </div>
            )}
            <div className="sheet-actions">
              <button className="btn" onClick={() => void load()} disabled={running}>Refresh</button>
              <button className="btn" onClick={close} disabled={running}>Close</button>
              <button className="btn primary" disabled={running || chosen.length === 0} onClick={() => void start()} data-testid="ship-go">
                {running ? "Shipping…" : `Ship ${chosen.length || ""}${info.deployCommand ? " & deploy" : ""}`}
              </button>
            </div>
          </>
        )}
        {info && tab === "history" && (
          <div className="ship-history">
            <div className="subhead">Deploys</div>
            {info.deploys.length === 0 && <p className="muted pad">No ships yet.</p>}
            {[...info.deploys].reverse().slice(0, 15).map((d) => (
              <div key={d.tag + d.at} className="deploy-row" data-testid="deploy-row">
                <code>{d.tag}</code>
                <span className="muted small">{ago(d.at)} · {d.machine} · {d.branches.length} feature{d.branches.length === 1 ? "" : "s"}</span>
                <span className="small" style={{ color: d.result === "deploy_failed" ? "var(--red)" : d.result === "deploy_skipped" ? "var(--muted)" : "var(--green)" }}>{d.result.replace("_", " ")}</span>
                {info.deployCommand && d.result === "deployed" && <button className="link" onClick={() => redeploy(d.tag)}>redeploy</button>}
              </div>
            ))}
            {deployPending && (
              <button className="btn small primary" onClick={() => void retryDeploy()} disabled={running} data-testid="retry-deploy">Retry deploy of main</button>
            )}
            {info.deployCommand && deployedCount > 1 && (
              <button className="btn small danger" onClick={() => redeploy()} disabled={running} data-testid="redeploy-previous">Redeploy previous</button>
            )}
            <div className="subhead">Shipped features</div>
            {history.length === 0 && <p className="muted pad">Nothing shipped with fleet yet.</p>}
            {history.map((c) => (
              <div key={c.commit} className="deploy-row">
                <span>{c.subject}</span>
                <span className="muted small">{relTime(c.at)}</span>
                <button className="link" onClick={() => undo(c)} data-testid="undo-feature">undo</button>
              </div>
            ))}
            {(running || run?.log) && <pre className="ship-log">{(run?.log ?? "").replace(/\x1b\[[0-9;]*[A-Za-z]/g, "").slice(-20000)}</pre>}
            <div className="sheet-actions"><button className="btn" onClick={close} disabled={running}>Close</button></div>
          </div>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- Add project from GitHub

export function ReposDialog() {
  const open = useSync((s) => s.reposOpen);
  const [repos, setRepos] = useState<GhRepo[] | null>(null);
  const [q, setQ] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  useEffect(() => {
    if (!open) return;
    setRepos(null);
    setErr(null);
    invoke<GhRepo[]>("gh_repos").then(setRepos).catch((e) => setErr(String(e)));
  }, [open]);
  if (!open) return null;
  const close = () => useSync.setState({ reposOpen: false });
  const shown = (repos ?? []).filter((r) => r.nameWithOwner.toLowerCase().includes(q.toLowerCase()));
  const clone = async (r: GhRepo) => {
    const parent = await (await backend()).pickFolder();
    if (!parent) return;
    setBusy(r.nameWithOwner);
    try {
      const p = await invoke<{ id: string }>("sync_clone", { repo: r.nameWithOwner, parent });
      useFleet.setState({ editorProjectId: p.id });
      useFleet.getState().showToast(`Added ${r.nameWithOwner} — sessions from your other Macs appear under “On other computers”`);
      close();
    } catch (e) {
      setErr(String(e));
    }
    setBusy(null);
  };
  return (
    <div className="overlay" onMouseDown={close}>
      <div className="sheet" role="dialog" aria-label="Add project from GitHub" data-testid="repos-dialog" onMouseDown={(e) => e.stopPropagation()} onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") close(); }}>
        <h3>Add project from GitHub</h3>
        <input className="field" autoFocus placeholder="Filter repositories" value={q} onChange={(e) => setQ(e.target.value)} />
        {err && <div className="git-sync-errors">{err}</div>}
        <div className="repo-list">
          {!repos && !err && <p className="muted">Loading your repositories…</p>}
          {shown.map((r) => (
            <button key={r.nameWithOwner} className="repo-row" disabled={!!busy} onClick={() => void clone(r)} data-testid="repo-row">
              <span>{r.nameWithOwner}{r.isPrivate && <span className="muted small"> · private</span>}</span>
              <span className="muted small">{busy === r.nameWithOwner ? "cloning…" : r.description ?? ""}</span>
            </button>
          ))}
        </div>
        <p className="muted small">You pick the folder to clone into; fleet then loads its sessions from GitHub.</p>
        <div className="sheet-actions"><button className="btn" onClick={close}>Close</button></div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- Actions cost guard

export function WorkflowGuard({ projectId }: { projectId: string }) {
  const [found, setFound] = useState<WorkflowFinding[] | null>(null);
  const [show, setShow] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  useEffect(() => {
    invoke<WorkflowFinding[]>("sync_workflows", { projectId }).then(setFound).catch(() => setFound([]));
  }, [projectId]);
  if (!found || found.length === 0) return null;
  if (done) return <div className="workflow-guard ok small">✓ Limited on {done}; it goes live with the next Ship.</div>;
  return (
    <div className="workflow-guard" data-testid="workflow-guard">
      <div>
        ⚠ GitHub Actions run on every branch push ({found.map((f) => f.path.replace(".github/workflows/", "")).join(", ")}). fleet pushes after every
        prompt, so that costs Actions minutes. Limit them to main and ship tags? fleet runs your checks locally at Ship.
      </div>
      <div className="err-actions">
        <button className="btn small" onClick={() => setShow((v) => !v)} data-testid="workflow-review">{show ? "Hide change" : "Show change"}</button>
        <button className="btn small primary" disabled={!show} title={show ? undefined : "Review the change first"} data-testid="workflow-apply" onClick={async () => {
          try {
            setDone(await invoke<string>("sync_limit_workflows", { projectId }));
          } catch (e) {
            useFleet.getState().showToast(String(e), "error");
          }
        }}>Limit to main</button>
      </div>
      {show && found.map((f) => (
        <div key={f.path} className="wf-diff">
          <div className="muted small">{f.path}</div>
          <div className="wf-cols">
            <pre>{f.before}</pre>
            <pre>{f.after}</pre>
          </div>
        </div>
      ))}
    </div>
  );
}

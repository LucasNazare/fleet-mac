// GitHub sync UI: session badges, the setup dialog, the fleet.yml editor and the git panel header.
import { useEffect, useMemo, useRef, useState } from "react";
import type { Session, SyncCheck, YmlError } from "../types";
import { useFleet } from "../store/store";
import { useSync } from "../store/sync";
import { useGit } from "../store/git";
import { invoke } from "../ipc";
import * as pool from "../terminal/termPool";
import { relTime } from "./git/util";
import { confirmDialog } from "./Dialogs";
import { WorkflowGuard, fixConflict } from "./ShipUi";

const BADGE: Record<string, { text: (s: Session) => string; color: string }> = {
  pushed: { text: () => "✓ pushed", color: "var(--green)" },
  waiting: { text: (s) => `↑ ${s.sync?.waiting || 1} waiting`, color: "var(--amber)" },
  conflict: { text: () => "⚠ conflict", color: "var(--red)" },
  paused: { text: () => "⏸ paused", color: "var(--muted)" },
  syncing: { text: () => "↻ syncing", color: "var(--blue)" },
  taken_over: { text: () => "⇄ taken over", color: "var(--amber)" },
  error: { text: () => "✕ sync failed", color: "var(--red)" },
  new: { text: () => "new branch on first prompt", color: "var(--muted)" },
  behind: { text: () => "↓ behind GitHub", color: "var(--amber)" },
};

/** "✓ pushed" / "↑ 1 waiting" / "⚠ conflict" / "⏸ paused" on a session row. */
export function SyncBadge({ s }: { s: Session }) {
  const b = s.sync ? BADGE[s.sync.badge] : null;
  if (!b || s.remote) return null;
  const retry = s.sync!.badge === "waiting" || s.sync!.badge === "error" || s.sync!.badge === "behind";
  const taken = s.sync!.badge === "taken_over";
  const conflict = s.sync!.badge === "conflict" && !!s.branch && s.state !== "working";
  const tip = [s.branch && `⎇ ${s.branch}`, s.sync!.detail, s.sync!.lastPushAt && `pushed ${relTime(s.sync!.lastPushAt / 1000)}`, retry && "Click to retry now", taken && "Click to take it over", conflict && "Click to fix it with Claude"].filter(Boolean).join("\n");
  const takeOver = () =>
    confirmDialog({
      title: `Take over ${s.glyph} · ${s.title}?`,
      body: <>{s.sync?.detail?.split(" — ")[0] ?? "Another Mac has it open"}. This Mac catches up with what it pushed and continues from here; the other Mac stops pushing.</>,
      confirmLabel: "Take over",
      danger: true,
      onConfirm: () => void invoke("sync_take_over", { sessionId: s.id }),
    });
  return (
    <span
      className={`sync-badge ${retry || taken || conflict ? "clickable" : ""}`}
      style={{ color: b.color }}
      title={tip}
      data-testid="sync-badge"
      data-badge={s.sync!.badge}
      onClick={retry ? (e) => { e.stopPropagation(); void invoke("sync_session_now", { sessionId: s.id }); } : taken ? (e) => { e.stopPropagation(); takeOver(); } : conflict ? (e) => { e.stopPropagation(); void fixConflict(s.projectId, s.branch, s.pendingQuestion); } : undefined}
    >
      {taken && s.sync?.detail ? `⇄ ${s.sync.detail.split(" — ")[0].replace(/^Active on /, "active on ")}` : b.text(s)}
    </span>
  );
}

/** Status bar: "✓ synced 20s ago" / "offline · 3 waiting" / "⏸ sync paused". */
export function SyncStatusItem({ now }: { now: number }) {
  const status = useSync((s) => s.status);
  const [menu, setMenu] = useState<{ left: number; bottom: number } | null>(null);
  if (!status.anySynced) return null;
  let text: string;
  let color = "var(--muted)";
  const problems = Object.entries(status.problems ?? {});
  if (status.pausedAll) text = "⏸ sync paused";
  else if (problems.length) {
    text = `⚠ GitHub: ${problems.length === 1 ? problems[0][0] : `${problems.length} projects`}`;
    color = "var(--red)";
  } else if (status.offline) {
    text = `offline${status.waiting ? ` · ${status.waiting} waiting` : ""}`;
    color = "var(--amber)";
  } else if (status.waiting) {
    text = `↑ ${status.waiting} waiting`;
    color = "var(--amber)";
  } else text = status.lastSyncAt ? `✓ synced ${relTime(Math.min(status.lastSyncAt, now) / 1000)}` : "✓ synced";
  const openLog = () => {
    setMenu(null);
    useFleet.getState().setUi({ rightHidden: false });
    useGit.setState({ outputOpen: true, autoOnly: true });
  };
  return (
    <span className="sync-status-wrap">
      <button className="status-count" style={{ color }} onClick={(e) => {
        const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
        setMenu((m) => (m ? null : { left: Math.max(8, Math.min(r.left, window.innerWidth - 240)), bottom: window.innerHeight - r.top + 4 }));
      }} data-testid="sync-status" title={problems.length ? problems.map(([p, m]) => `${p}: ${m}`).join("\n") : "GitHub sync"}>
        {text}
      </button>
      {menu && (
        <div className="menu sync-menu" style={menu} onMouseLeave={() => setMenu(null)}>
          {problems.map(([p, m]) => <div key={p} className="menu-note small" data-testid="sync-problem"><b>{p}</b>: {m}</div>)}
          <button className="menu-item" onClick={openLog}>Sync log</button>
          <button className="menu-item" onClick={() => { setMenu(null); void invoke("sync_now"); }}>Sync now</button>
          <button className="menu-item" onClick={() => { setMenu(null); void invoke("sync_set_paused", { projectId: null, paused: !status.pausedAll }); }} data-testid="pause-all">
            {status.pausedAll ? "Resume sync (all projects)" : "Pause sync (all projects)"}
          </button>
        </div>
      )}
    </span>
  );
}

/** Small marker next to a synced project's name. */
export function ProjectSyncPill({ projectId }: { projectId: string }) {
  const c = useSync((s) => s.configs[projectId]);
  const paused = useFleet((s) => s.projects.find((p) => p.id === projectId)?.syncPaused ?? false);
  if (!c?.enabled) return null;
  const errors = c.errors.length > 0;
  return (
    <span
      className="sync-pill"
      data-testid="project-sync-pill"
      style={{ color: errors ? "var(--red)" : paused || c.paused ? "var(--muted)" : "var(--green)" }}
      title={errors ? "fleet.yml has errors — using the last valid version" : paused ? "GitHub sync paused on this Mac" : `Synced with ${c.originUrl}`}
    >
      {errors ? "⚠" : paused || c.paused ? "⏸" : "⇅"}
    </span>
  );
}

/** "26 unsaved changes in the project folder — Keep as sessions". */
export function LocalWorkBanner({ projectId }: { projectId: string }) {
  const c = useSync((s) => s.configs[projectId]);
  const presync = useFleet((s) => Object.values(s.sessions).filter((x) => x.projectId === projectId && !x.archived && !x.remote && !x.worktreePath).length);
  const [busy, setBusy] = useState(false);
  if (!c?.enabled) return null;
  const n = c.localChanges;
  if (n === 0 && c.localAhead === 0 && presync === 0) return null;
  const parts = [
    n > 0 && `${n} unsaved change${n === 1 ? "" : "s"}`,
    c.localAhead > 0 && `${c.localAhead} unsynced commit${c.localAhead === 1 ? "" : "s"}`,
    presync > 0 && `${presync} session${presync === 1 ? "" : "s"} from before sync`,
  ].filter(Boolean);
  const go = () => {
    confirmDialog({
      title: "Keep this work as sessions?",
      body: (
        <>
          fleet moves {parts.join(", ")} out of the project folder into synced sessions. Files a session edited go with that
          session; everything else becomes a <b>Local changes</b> session. All of it is saved to GitHub, and the project folder
          goes back to matching what's live. Nothing is deleted.
        </>
      ),
      confirmLabel: "Keep as sessions",
      onConfirm: async () => {
        setBusy(true);
        const { cols, rows } = pool.currentSize();
        try {
          const r = await invoke<{ movedSessions: number; newSession?: string | null; files: number }>("sync_adopt_local", { projectId, cols, rows });
          if (r.newSession) useFleet.getState().select(r.newSession);
          useFleet.getState().showToast("Saved as sessions — syncing to GitHub");
        } catch (e) {
          useFleet.getState().showToast(String(e), "error");
        }
        setBusy(false);
      },
    });
  };
  return (
    <div className="local-work" data-testid="local-work">
      <span>{parts.join(" · ")} in the project folder</span>
      <button className="btn small primary" disabled={busy} onClick={go} data-testid="keep-as-sessions">{busy ? "Moving…" : "Keep as sessions"}</button>
    </div>
  );
}

// ---------------------------------------------------------------- setup dialog

export function SyncSetupDialog() {
  const projectId = useSync((s) => s.setup);
  const project = useFleet((s) => s.projects.find((p) => p.id === projectId));
  const [check, setCheck] = useState<SyncCheck | null>(null);
  const [busy, setBusy] = useState(false);
  const [repoForm, setRepoForm] = useState(false);
  const [repoName, setRepoName] = useState("");
  const [repoPrivate, setRepoPrivate] = useState(true);
  const [creating, setCreating] = useState(false);
  const run = async () => {
    if (!projectId) return;
    setBusy(true);
    try {
      setCheck(await invoke<SyncCheck>("sync_check", { projectId }));
    } catch (e) {
      useFleet.getState().showToast(String(e), "error");
    }
    setBusy(false);
  };
  useEffect(() => {
    setCheck(null);
    setRepoForm(false);
    setRepoName((project?.name ?? "").replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, ""));
    setRepoPrivate(true);
    void run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);
  if (!projectId || !project) return null;
  const close = () => useSync.setState({ setup: null });
  const terminal = async (action: "install" | "login") => {
    const { cols, rows } = pool.currentSize();
    try {
      const id = await invoke<string>("gh_terminal", { action, cols, rows });
      useFleet.setState((x) => ({ terminals: { ...x.terminals, [id]: action === "install" ? "Install GitHub CLI" : "Sign in to GitHub" } }));
      useFleet.getState().openProcess(id);
      close();
    } catch (e) {
      useFleet.getState().showToast(String(e), "error");
    }
  };
  const step = (ok: boolean | null, label: React.ReactNode, action?: React.ReactNode) => (
    <div className="sync-step">
      <span style={{ color: ok === null ? "var(--muted)" : ok ? "var(--green)" : "var(--amber)" }}>{ok === null ? "○" : ok ? "✓" : "●"}</span>
      <span className="sync-step-label">{label}</span>
      {action}
    </div>
  );
  const c = check;
  // No origin yet: GitHub is where the repo gets created, so gh is needed too.
  const ghNeeded = !c || !c.hasOrigin || c.isGithub;
  const createRepo = async () => {
    if (!projectId) return;
    setCreating(true);
    try {
      const r = await invoke<{ url: string; branch: string; skipped: string[] }>("sync_create_repo", { projectId, name: repoName, private: repoPrivate });
      setRepoForm(false);
      useFleet.getState().showToast(`Created ${r.url || repoName} and pushed ${r.branch}${r.skipped.length ? ` (kept out: ${r.skipped.join(", ")})` : ""}`);
      await run();
    } catch (e) {
      useFleet.getState().showToast(String(e).replace(/^Error: /, ""), "error");
    }
    setCreating(false);
  };
  return (
    <div className="overlay" onMouseDown={close}>
      <div className="sheet" role="dialog" aria-label="GitHub sync" data-testid="sync-setup" onMouseDown={(e) => e.stopPropagation()} onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") close(); }}>
        <h3>GitHub sync · {project.name}</h3>
        <p className="muted small">
          GitHub becomes the source of truth. Every session works on its own branch, committed and pushed after each finished
          prompt, so you can continue on any Mac. main changes only through Ship.
        </p>
        {c?.enabled && <WorkflowGuard projectId={projectId} />}
        {!c ? (
          <p className="muted">{busy ? "Checking…" : ""}</p>
        ) : (
          <div className="sync-steps">
            {step(c.isRepo, "Git repository")}
            {step(
              c.hasOrigin,
              c.hasOrigin ? <>origin <span className="muted">{c.originUrl}</span></> : "An `origin` remote (your GitHub repo)",
              !c.hasOrigin && c.gh.authed && !repoForm && <button className="btn small primary" onClick={() => setRepoForm(true)} data-testid="create-repo">Create GitHub repo</button>,
            )}
            {!c.hasOrigin && repoForm && (
              <div className="sync-repo-form" data-testid="create-repo-form">
                <label className="small">
                  github.com/{c.gh.user ?? "you"}/
                  <input value={repoName} onChange={(e) => setRepoName(e.target.value)} spellCheck={false} autoFocus aria-label="Repo name" data-testid="repo-name"
                    onKeyDown={(e) => { if (e.key === "Enter" && repoName.trim() && !creating) void createRepo(); }} />
                </label>
                <label className="small">
                  <input type="checkbox" checked={repoPrivate} onChange={(e) => setRepoPrivate(e.target.checked)} /> Private (only you can see it)
                </label>
                <p className="muted small">
                  Creates the repo, makes the first commit on main if there's none yet (.env files and keys stay out), and pushes it.
                </p>
                <div className="sheet-actions">
                  <button className="btn small" onClick={() => setRepoForm(false)} disabled={creating}>Cancel</button>
                  <button className="btn small primary" onClick={() => void createRepo()} disabled={creating || !repoName.trim()} data-testid="create-repo-go">
                    {creating ? "Creating…" : "Create & push"}
                  </button>
                </div>
              </div>
            )}
            {ghNeeded && step(c.gh.installed, "GitHub CLI (gh)", !c.gh.installed && <button className="btn small" onClick={() => void terminal("install")}>Install</button>)}
            {ghNeeded && c.gh.installed && step(c.gh.authed, c.gh.authed ? <>Signed in{c.gh.user ? ` as ${c.gh.user}` : ""}</> : "Signed in to GitHub", !c.gh.authed && <button className="btn small primary" onClick={() => void terminal("login")} data-testid="gh-login">Sign in</button>)}
            {step(c.enabled ? true : c.mainHasFleetYml ? false : null, c.enabled ? "fleet.yml with sync enabled" : c.mainHasFleetYml ? `fleet.yml on ${c.defaultBranch} (sync not enabled)` : `fleet.yml (not on ${c.defaultBranch} yet)`)}
          </div>
        )}
        <div className="sheet-actions">
          <button className="btn" onClick={() => void run()} disabled={busy}>Re-check</button>
          <button className="btn" onClick={close}>Close</button>
          <button
            className="btn primary"
            data-testid="sync-create-config"
            disabled={!c || !c.isRepo || !c.hasOrigin}
            onClick={() => { close(); void useSync.getState().openEditor(projectId); }}
          >
            {c?.enabled ? "Edit fleet.yml…" : c?.mainHasFleetYml ? "Enable in fleet.yml…" : "Create fleet.yml…"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- fleet.yml editor

export function ConfigEditor() {
  const ed = useSync((s) => s.editor);
  const [text, setText] = useState("");
  const [errors, setErrors] = useState<YmlError[]>([]);
  const [saving, setSaving] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);
  const gutter = useRef<HTMLDivElement>(null);
  const project = useFleet((s) => s.projects.find((p) => p.id === ed?.projectId));

  useEffect(() => {
    if (!ed) return;
    setText(ed.text);
    setTimeout(() => ed.line && goto(ed.line), 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ed]);
  useEffect(() => {
    if (!ed) return;
    const t = setTimeout(() => void invoke<YmlError[]>("sync_validate", { text }).then(setErrors).catch(() => {}), 250);
    return () => clearTimeout(t);
  }, [text, ed]);
  const lines = useMemo(() => text.split("\n").length, [text]);
  const bad = useMemo(() => new Set(errors.map((e) => e.line)), [errors]);
  if (!ed || !project) return null;

  function goto(line: number) {
    const el = area.current;
    if (!el) return;
    const all = el.value.split("\n");
    const pos = all.slice(0, Math.max(0, line - 1)).reduce((n, l) => n + l.length + 1, 0);
    el.focus();
    el.setSelectionRange(pos, pos + (all[line - 1]?.length ?? 0));
    el.scrollTop = Math.max(0, (line - 4) * 17);
  }
  const close = () => useSync.setState({ editor: null });
  const save = async () => {
    setSaving(true);
    try {
      await invoke("sync_save_config", { projectId: ed.projectId, text });
      close();
    } catch (e) {
      useFleet.getState().showToast(String(e), "error");
    }
    setSaving(false);
  };
  return (
    <div className="overlay" onMouseDown={close}>
      <div className="sheet config-editor" role="dialog" aria-label="fleet.yml" data-testid="config-editor" onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") close();
          if (e.key === "Enter" && e.metaKey && errors.length === 0) void save();
        }}>
        <h3>fleet.yml · {project.name}</h3>
        <p className="muted small">Saved to a <code>fleet/config-…</code> branch; this Mac uses it right away and main gets it with the next Ship. Env var names only — never values.</p>
        <div className="yml-wrap">
          <div className="yml-gutter" ref={gutter} aria-hidden>
            {Array.from({ length: lines }, (_, i) => (
              <div key={i} className={bad.has(i + 1) ? "bad" : ""}>{i + 1}</div>
            ))}
          </div>
          <textarea
            ref={area}
            className="yml-text"
            spellCheck={false}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onScroll={(e) => { if (gutter.current) gutter.current.scrollTop = (e.target as HTMLTextAreaElement).scrollTop; }}
            onKeyDown={(e) => {
              if (e.key === "Tab") {
                e.preventDefault();
                const el = e.currentTarget;
                const { selectionStart: a, selectionEnd: b } = el;
                setText(text.slice(0, a) + "  " + text.slice(b));
                setTimeout(() => el.setSelectionRange(a + 2, a + 2), 0);
              }
            }}
            data-testid="yml-text"
          />
        </div>
        <div className="yml-errors" data-testid="yml-errors">
          {errors.length === 0 ? <span className="muted small">✓ valid</span> : errors.map((e, i) => (
            <button key={i} className="yml-error" onClick={() => goto(e.line)}>line {e.line}: {e.message}</button>
          ))}
        </div>
        <div className="sheet-actions">
          <button className="btn" onClick={close}>Cancel</button>
          <button className="btn primary" disabled={saving || errors.length > 0} onClick={() => void save()} data-testid="yml-save">
            {saving ? "Saving…" : "Save ⌘↵"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- git panel header

/** "main = production · last deploy 14:02", fleet.yml errors, excluded files. */
export function GitSyncHeader({ projectId, session }: { projectId: string; session: Session | null }) {
  const c = useSync((s) => s.configs[projectId]);
  if (!c?.enabled && !(c && c.errors.length)) return null;
  const d = c.lastDeploy;
  const at = d ? new Date(d.at) : null;
  const time = at ? at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : null;
  return (
    <div className="git-sync" data-testid="git-sync-header">
      {c.enabled && (
        <div className="git-sync-line">
          <span>{c.defaultBranch} = production</span>
          <span className="muted"> · {d ? <>last deploy {time} ({d.tag})</> : "no deploys yet"}</span>
          {c.undeployed && <button className="chip-warn" title="This Mac already uses it" onClick={() => void useSync.getState().openEditor(projectId)} data-testid="config-pending">settings go live with the next Ship</button>}
        </div>
      )}
      {c.errors.length > 0 && (
        <div className="git-sync-errors" role="alert" data-testid="fleet-yml-errors">
          <div>⚠ fleet.yml has errors — fleet keeps using the last valid version.</div>
          {c.errors.map((e, i) => (
            <button key={i} className="yml-error" onClick={() => void useSync.getState().openEditor(projectId, e.line)}>line {e.line}: {e.message}</button>
          ))}
        </div>
      )}
      {session?.sync?.excluded?.length ? (
        <div className="git-sync-excluded" data-testid="excluded-files">
          ⚠ Never auto-committed: {session.sync.excluded.join(", ")}
        </div>
      ) : null}
    </div>
  );
}

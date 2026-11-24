import { useEffect, useRef, useState } from "react";
import { useFleet, orderedProjects } from "../store/store";
import * as pool from "../terminal/termPool";
import { useSettings } from "../store/settings";
import { newRemoteSession } from "./RemoteRows";

const MODES = [
  { id: "default", label: "Default (ask for permissions)" },
  { id: "acceptEdits", label: "Accept edits" },
  { id: "plan", label: "Plan" },
  { id: "auto", label: "Auto" },
];

export function NewSessionSheet() {
  const { projects, sessions, activeSessionId, sheetProjectId, setUi, newSession } = useFleet();
  const current = sheetProjectId ?? (activeSessionId ? sessions[activeSessionId]?.projectId : null) ?? orderedProjects(projects)[0]?.id;
  const [projectId, setProjectId] = useState(current ?? "");
  const [prompt, setPrompt] = useState("");
  const [mode, setMode] = useState(useSettings.getState().settings.claude.permissionMode || "default");
  const [worktree, setWorktree] = useState(false);
  const [remote, setRemote] = useState("");
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);

  const conns = projects.find((p) => p.id === projectId)?.sshConnections ?? [];
  const conn = conns.find((c) => c.id === remote) ?? null;

  const close = () => setUi({ sheetOpen: false, sheetProjectId: null });
  const start = async () => {
    if (!projectId || busy) return;
    if (conn) {
      // Close first: connecting may need the login terminal (password / 2FA).
      close();
      await newRemoteSession(projectId, conn, prompt, mode);
      return;
    }
    setBusy(true);
    await newSession(projectId, prompt, mode, pool.currentSize(), worktree);
    setBusy(false);
  };

  return (
    <div className="overlay" onMouseDown={close}>
      <div className="sheet" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label="New session"
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") close();
          if (e.key === "Enter" && (e.metaKey || !e.shiftKey) && (e.target as HTMLElement).tagName !== "SELECT") { e.preventDefault(); void start(); }
        }}>
        <h3>New Claude Code session <span className="muted small">⌘N skips this sheet</span></h3>
        {projects.length === 0 ? (
          <p className="muted">Add a project first.</p>
        ) : (
          <>
            <label>Project
              <select value={projectId} onChange={(e) => { setProjectId(e.target.value); setRemote(""); }} data-testid="sheet-project">
                {orderedProjects(projects).map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
              </select>
            </label>
            <label>First prompt <span className="muted">(optional)</span>
              <textarea ref={ref} rows={3} value={prompt} onChange={(e) => setPrompt(e.target.value)}
                placeholder="What should Claude do?  ↵ to start, ⇧↵ for a new line" data-testid="sheet-prompt" />
            </label>
            <label>Permission mode
              <select value={mode} onChange={(e) => setMode(e.target.value)}>
                {MODES.map((m) => <option key={m.id} value={m.id}>{m.label}</option>)}
              </select>
            </label>
            {conns.length > 0 && (
              <label>Runs on
                <select value={remote} onChange={(e) => setRemote(e.target.value)} data-testid="sheet-remote">
                  <option value="">This Mac</option>
                  {conns.map((c) => <option key={c.id} value={c.id}>⇄ {c.name} ({c.host}:{c.remotePath})</option>)}
                </select>
              </label>
            )}
            {!conn && (
              <label className="check" title="git worktree add .fleet/worktrees/<slug> -b fleet/<slug>">
                <input type="checkbox" checked={worktree} onChange={(e) => setWorktree(e.target.checked)} data-testid="sheet-worktree" /> Run in its own git worktree
              </label>
            )}
          </>
        )}
        <div className="sheet-actions">
          <button className="btn" onClick={close}>Cancel</button>
          <button className="btn primary" disabled={!projectId || busy} onClick={() => void start()} data-testid="sheet-start">
            {busy ? "Starting…" : "Start session ↵"}
          </button>
        </div>
      </div>
    </div>
  );
}

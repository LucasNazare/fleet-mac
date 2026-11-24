import { useEffect, useLayoutEffect, useRef } from "react";
import type { Session } from "../types";
import { useFleet } from "../store/store";
import { Chip, ContextMeter, StateLabel, useNow } from "./common";
import { invoke } from "../ipc";
import * as pool from "../terminal/termPool";
import { ConsolePane } from "./ConsolePane";
import { connectSsh } from "./RemoteRows";
import { SessionUsage } from "./Usage";

function hhmm(ms: number) {
  return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export function SessionView({ s, pane, focused }: { s: Session; pane: 0 | 1; focused: boolean }) {
  const host = useRef<HTMLDivElement>(null);
  const project = useFleet((st) => st.projects.find((p) => p.id === s.projectId));
  const conn = project?.sshConnections?.find((c) => c.id === s.remote) ?? null;
  const connState = useFleet((st) => (s.remote ? st.ssh[s.remote]?.state ?? "disconnected" : null));
  const now = useNow(s.metrics?.turnStarted != null ? 1000 : 5000);

  // Attach this session's terminal to the host while shown.
  useLayoutEffect(() => {
    const el = host.current;
    if (!el) return;
    pool.mount(s.id, el, focused);
    void pool.connect(s.id);
    const ro = new ResizeObserver(() => pool.fitNow(s.id));
    ro.observe(el);
    return () => {
      ro.disconnect();
      pool.unmount(s.id, el);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.id]);

  useEffect(() => {
    if (focused) pool.focus(s.id);
  }, [focused, s.id]);

  // When the backend restarts the PTY, reconnect.
  useEffect(() => {
    if (s.alive) void pool.connect(s.id);
  }, [s.alive, s.id]);

  const restart = async () => {
    const { cols, rows } = pool.currentSize();
    pool.reset(s.id);
    await invoke("restart_session", { id: s.id, cols, rows }).catch((e) => useFleet.getState().showToast(String(e), "error"));
    await pool.connect(s.id);
    pool.focus(s.id);
  };

  const prev = s.titleHistory[s.titleHistory.length - 1];
  const tooltip = [prev ? `was: ${prev}` : "", ...s.recap.map((r) => `• ${r}`)].filter(Boolean).join("\n");

  return (
    <div
      className={`session-view ${focused ? "focused" : ""}`}
      style={{ borderTopColor: s.color }}
      onMouseDown={() => pane !== useFleet.getState().focusedPane && useFleet.setState({ focusedPane: pane })}
      data-testid="session-view"
    >
      <div className="session-header">
        <Chip s={s} size={40} />
        <div className="hdr-text">
          <div className="hdr-title" title={tooltip || undefined}>{s.title}</div>
          <div className="hdr-meta muted">
            {project?.name ?? "?"}
            {conn && <span className="remote-badge"> · ⇄ {conn.name}:{conn.remotePath}</span>}
            {s.branch && <> · {s.branch}</>}
            {s.worktreePath && <> · worktree</>}
            {" · "}started {hhmm(s.createdAt)}
          </div>
        </div>
        <div className="hdr-right">
          <StateLabel s={{ ...s, pendingQuestion: null }} now={now} />
          <SessionUsage s={s} now={now} />
          <ContextMeter pct={s.contextPct} />
        </div>
      </div>
      {s.recap.length > 0 && (
        <div className="recap muted" title={s.recap.join("\n")}>
          {s.recap.slice(0, 2).join(" · ")}
        </div>
      )}
      {s.pendingQuestion && (s.state === "needs_permission" || s.state === "needs_reply") && (
        <div className="question-bar">● {s.pendingQuestion}</div>
      )}
      {conn && connState !== "connected" && (
        <div className="question-bar">
          ⇄ Not connected to {conn.name} — Claude's tools can't reach the server.{" "}
          <button className="btn small" onClick={() => void connectSsh(conn)}>{connState === "connecting" ? "Connecting…" : "Reconnect"}</button>
        </div>
      )}
      {!s.alive && (
        <div className="exited-bar">
          {s.archived ? "Archived." : s.state === "failed" ? `claude exited with code ${s.exitCode ?? "?"}.` : "claude exited."}
          <button className="btn small" onClick={() => void restart()}>↻ Resume (claude --resume)</button>
        </div>
      )}
      <div className="term-host" ref={host} />
      <ConsolePane s={s} />
    </div>
  );
}

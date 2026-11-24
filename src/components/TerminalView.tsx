import { useLayoutEffect, useRef } from "react";
import { useFleet } from "../store/store";
import { invoke } from "../ipc";
import * as pool from "../terminal/termPool";

/** A standalone terminal in the center pane (remote shells, SSH login prompts). */
export function TerminalView({ id }: { id: string }) {
  const title = useFleet((s) => s.terminals[id] ?? id);
  const host = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = host.current;
    if (!el) return;
    pool.mount(id, el, true);
    void pool.connect(id);
    const ro = new ResizeObserver(() => pool.fitNow(id));
    ro.observe(el);
    return () => {
      ro.disconnect();
      pool.unmount(id, el);
    };
  }, [id]);
  const isLogin = id.startsWith("ssh:");
  return (
    <div className="session-view process-view focused" data-testid="terminal-view">
      <div className="session-header">
        <span className="proc-icon">⇄</span>
        <div className="hdr-text">
          <div className="hdr-title">{title}</div>
          <div className="hdr-meta muted">{isLogin ? "Answer any password, 2FA or host-key prompt here. fleet never stores it." : "Shell on the server"}</div>
        </div>
        {!isLogin && (
          <div className="proc-actions">
            <button className="btn small" onClick={() => { void invoke("terminal_close", { id }); pool.dispose(id); useFleet.setState((s) => { const t = { ...s.terminals }; delete t[id]; return { terminals: t, selectedProcess: null }; }); }}>Close</button>
          </div>
        )}
      </div>
      <div className="term-host" ref={host} />
    </div>
  );
}

import { useLayoutEffect, useRef } from "react";
import { useFleet } from "../store/store";
import { invoke } from "../ipc";
import { openConsoleTab } from "../commands";
import type { Session } from "../types";
import * as pool from "../terminal/termPool";

/** ⌃N console under a session: tabs of shells in its folder (or on its server). */
export function ConsolePane({ s }: { s: Session }) {
  const c = useFleet((st) => st.consoles[s.id]);
  const conn = useFleet((st) => (s.remote ? st.projects.flatMap((p) => p.sshConnections).find((x) => x.id === s.remote) : null));
  const host = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = host.current;
    const id = c?.active;
    if (!el || !id) return;
    pool.mount(id, el, true);
    void pool.connect(id);
    const ro = new ResizeObserver(() => pool.fitNow(id));
    ro.observe(el);
    return () => {
      ro.disconnect();
      pool.unmount(id, el);
    };
  }, [c?.active]);

  if (!c?.open) return null;
  const set = (patch: Partial<typeof c>) => useFleet.setState((st) => ({ consoles: { ...st.consoles, [s.id]: { ...st.consoles[s.id], ...patch } } }));
  const close = (id: string) => {
    void invoke("terminal_close", { id });
    pool.dispose(id);
    const tabs = c.tabs.filter((t) => t !== id);
    set({ tabs, open: tabs.length > 0, active: c.active === id ? tabs[tabs.length - 1] ?? null : c.active });
    if (!tabs.length) pool.focus(s.id);
  };
  const drag = (e: React.MouseEvent) => {
    e.preventDefault();
    const y0 = e.clientY;
    const h0 = c.height;
    const move = (ev: MouseEvent) => set({ height: Math.max(100, Math.min(window.innerHeight - 220, h0 - (ev.clientY - y0))) });
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      document.body.classList.remove("resizing-v");
      if (c.active) pool.fitNow(c.active);
    };
    document.body.classList.add("resizing-v");
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  return (
    <div className="console-pane" style={{ height: c.height }} data-testid="console-pane">
      <div className="console-resizer" onMouseDown={drag} role="separator" aria-orientation="horizontal" />
      <div className="console-tabs">
        <span className="muted small console-where">{conn ? `⇄ ${conn.name}:${conn.remotePath}` : "console"}</span>
        {c.tabs.map((id, i) => (
          <div key={id} className={`ctab ${id === c.active ? "on" : ""}`} onClick={() => set({ active: id })} data-testid="console-tab">
            shell {i + 1}
            <button className="etab-x" aria-label={`Close shell ${i + 1}`} onClick={(e) => { e.stopPropagation(); close(id); }}>×</button>
          </div>
        ))}
        <button className="icon-btn" title="New shell" aria-label="New shell" onClick={() => void openConsoleTab(s.id)} data-testid="console-add">+</button>
        <span className="spacer" />
        <button className="link small" onClick={() => { set({ open: false }); pool.focus(s.id); }}>hide ⌃N</button>
      </div>
      <div className="console-host" ref={host} />
    </div>
  );
}

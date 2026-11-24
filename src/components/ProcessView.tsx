import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useFleet } from "../store/store";
import { useNow } from "./common";
import { runLabel, isRunning, defLabel } from "./processUi";
import { restartProcess, startProcess, stopProcess } from "./ProcessRows";
import { openSendPicker } from "./SendPicker";
import * as pool from "../terminal/termPool";
import { backend } from "../ipc";
import { TerminalView } from "./TerminalView";

export function ProcessView() {
  const sel = useFleet((s) => s.selectedProcess);
  if (sel && (sel.startsWith("rterm:") || sel.startsWith("ssh:"))) return <TerminalView key={sel} id={sel} />;
  return <ProcessPane />;
}

function ProcessPane() {
  const { selectedProcess, projects, runs } = useFleet();
  const host = useRef<HTMLDivElement>(null);
  const now = useNow(1000);
  const [findOpen, setFindOpen] = useState(false);
  const [q, setQ] = useState("");
  const findRef = useRef<HTMLInputElement>(null);

  const parts = selectedProcess?.split(":") ?? [];
  const project = projects.find((p) => p.id === parts[1]);
  const def = project?.processes.find((d) => d.id === parts[2]);
  const run = selectedProcess ? runs[selectedProcess] : undefined;
  const key = project && def ? selectedProcess! : null;

  useLayoutEffect(() => {
    const el = host.current;
    if (!el || !key) return;
    pool.mount(key, el, true);
    void pool.connect(key);
    const ro = new ResizeObserver(() => pool.fitNow(key));
    ro.observe(el);
    return () => {
      ro.disconnect();
      pool.unmount(key, el);
    };
  }, [key]);

  useEffect(() => {
    const open = () => { setFindOpen(true); setTimeout(() => findRef.current?.select(), 0); };
    window.addEventListener("fleet:find", open);
    return () => window.removeEventListener("fleet:find", open);
  }, []);

  if (!project || !def || !key) {
    const anyProcs = projects.some((p) => p.processes.length);
    return (
      <div className="placeholder">
        <h2>Processes</h2>
        <p className="muted">
          {anyProcs ? "Pick a process in the sidebar to see its terminal." : "Dev servers and watchers belong to a project. Add one from a project's “+ Add process”."}
        </p>
      </div>
    );
  }

  const label = runLabel(run, now);
  const running = isRunning(run);
  return (
    <div className="session-view process-view focused" data-testid="process-view">
      <div className="session-header">
        <span className="proc-icon">▶</span>
        <div className="hdr-text">
          <div className="hdr-title">{defLabel(def)}</div>
          <div className="hdr-meta muted">
            {project.name} · <code>{def.command}</code>{def.cwd && def.cwd !== "." ? ` · in ${def.cwd}` : ""}
            {run?.pid ? ` · pid ${run.pid}` : ""}
          </div>
        </div>
        <div className="proc-actions">
          <span className="proc-status" style={{ color: label.color }}>{label.symbol} {label.text}</span>
          {run?.status === "running" && run.detectedPorts.map((p) => (
            <button key={p} className="btn small" onClick={() => void backend().then((b) => b.openUrl(`http://localhost:${p}`))}>
              Open :{p} ↗
            </button>
          ))}
          {running ? (
            <button className="btn small" onClick={() => void stopProcess(project.id, def.id)} title="⌘.">■ Stop</button>
          ) : (
            <button className="btn small primary" onClick={() => void startProcess(project.id, def.id)} data-testid="proc-start">▶ Start</button>
          )}
          <button className="btn small" onClick={() => void restartProcess(project.id, def.id)} title="⌘R">↻ Restart</button>
          <button className="btn small" onClick={() => pool.clear(key)}>Clear</button>
          <button className="btn small" onClick={() => openSendPicker(key)} title="Send the selected text to a session (⌘⇧S)">Send to session…</button>
        </div>
      </div>
      {findOpen && (
        <div className="find-bar">
          <input
            ref={findRef}
            autoFocus
            value={q}
            placeholder="Find in output"
            onChange={(e) => { setQ(e.target.value); pool.find(key, e.target.value); }}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter") pool.find(key, q, e.shiftKey);
              if (e.key === "Escape") { setFindOpen(false); pool.clearFind(key); pool.focus(key); }
            }}
          />
          <span className="muted small">↵ next · ⇧↵ previous · esc close</span>
        </div>
      )}
      {run?.detached && (
        <div className="question-bar">Left running by a previous fleet session — its output isn't available here. Stop it to free its port, or restart it to get a terminal.</div>
      )}
      {run?.status === "crashed" && (
        <div className="exited-bar">✕ {defLabel(def)} crashed (exit {run.exitCode ?? "?"}). Select the error and press ⌘⇧S to send it to a session.</div>
      )}
      <div className="term-host" ref={host} />
    </div>
  );
}

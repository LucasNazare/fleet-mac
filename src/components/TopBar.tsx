import { memo } from "react";
import { useFleet } from "../store/store";
import { useSync, shipProject } from "../store/sync";

export const TopBar = memo(function TopBar() {
  const { projects, sessions, activeSessionId, runs } = useFleet();
  const live = Object.values(sessions).filter((s) => !s.archived);
  const active = activeSessionId ? sessions[activeSessionId] : null;
  const project = active ? projects.find((p) => p.id === active.projectId) : null;
  const procs = Object.values(runs).filter((r) => r.status === "running").length;
  const anySynced = useSync((s) => Object.values(s.configs).some((c) => c.enabled));
  const shipping = useSync((s) => Object.values(s.runs).some((r) => r.running));
  return (
    <header className="topbar" data-tauri-drag-region>
      <span className="traffic-space" data-tauri-drag-region />
      <span className="app-name" data-tauri-drag-region>fleet</span>
      {project && <span className="muted" data-tauri-drag-region>/ {project.name}</span>}
      <span className="muted counts" data-tauri-drag-region>
        {live.length} agent{live.length === 1 ? "" : "s"} · {procs} process{procs === 1 ? "" : "es"}
      </span>
      <span className="spacer" data-tauri-drag-region />
      {anySynced && (
        <button className="btn small ship-btn" data-testid="ship-btn" title="Ship to main (⌘⇧D)" onClick={() => { const p = shipProject(); if (p) useSync.setState({ ship: p }); }}>
          {shipping ? "Shipping…" : "Ship ⌘⇧D"}
        </button>
      )}
      <span className="hints muted">
        <kbd>⌘K</kbd> jump to anything <kbd>⌘N</kbd> new session <kbd>⌘J</kbd> needs you <kbd>⌘E</kbd> editor <kbd>⌘,</kbd> settings
      </span>
    </header>
  );
});

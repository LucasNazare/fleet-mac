import { useState } from "react";
import type { Project } from "../types";
import { procKey } from "../types";
import { useFleet } from "../store/store";
import { backend, invoke } from "../ipc";
import { ContextMenu } from "./ContextMenu";
import { runLabel, isRunning, defLabel } from "./processUi";
import * as pool from "../terminal/termPool";
import { isSynced, useSync } from "../store/sync";

/** Synced projects run processes in the selected session's worktree. */
function baseSession(projectId: string): string | null {
  if (!isSynced(projectId)) return null;
  const st = useFleet.getState();
  const s = st.activeSessionId ? st.sessions[st.activeSessionId] : null;
  return s && s.projectId === projectId && s.worktreePath ? s.id : null;
}

export function startProcess(projectId: string, defId: string) {
  const { cols, rows } = pool.currentSize();
  return invoke("process_start", { projectId, defId, cols, rows, sessionId: baseSession(projectId) }).catch((e) => useFleet.getState().showToast(String(e), "error"));
}
export function stopProcess(projectId: string, defId: string) {
  return invoke("process_stop", { projectId, defId });
}
export function restartProcess(projectId: string, defId: string, sessionId?: string | null) {
  const { cols, rows } = pool.currentSize();
  return invoke("process_restart", { projectId, defId, cols, rows, sessionId: sessionId ?? null }).catch((e) => useFleet.getState().showToast(String(e), "error"));
}

export function ProcessRows({ p, now }: { p: Project; now: number }) {
  const { runs, selectedProcess, centerTab, openProcess, suggestions, setUi, activeSessionId, sessions } = useFleet();
  const synced = useSync((s) => !!s.configs[p.id]?.enabled);
  const active = activeSessionId ? sessions[activeSessionId] : null;
  const here = synced && active && active.projectId === p.id && active.worktreePath ? active : null;
  const addOrEdit = (def?: Project["processes"][number]) => (synced ? void useSync.getState().openEditor(p.id) : setUi({ procForm: { projectId: p.id, def } }));
  const [menu, setMenu] = useState<{ x: number; y: number; defId: string } | null>(null);
  const sugg = !p.processesOffered ? (suggestions[p.id] ?? []) : [];

  if (p.processes.length === 0 && sugg.length === 0) {
    return (
      <button className="ghost-row small" onClick={() => addOrEdit()}>
        + Add process
      </button>
    );
  }

  const addSuggestion = async (i: number) => {
    const d = sugg[i];
    await invoke("save_process", { projectId: p.id, def: { ...d, id: "" }, keepLocal: null });
    useFleet.setState((st) => ({ suggestions: { ...st.suggestions, [p.id]: sugg.filter((_, j) => j !== i) } }));
  };

  return (
    <>
      <div className="subhead">Processes</div>
      {p.processes.map((d) => {
        const key = procKey(p.id, d.id);
        const run = runs[key];
        const label = runLabel(run, now);
        const running = isRunning(run);
        const selected = selectedProcess === key && centerTab === "process";
        return (
          <div
            key={d.id}
            className={`proc-row ${selected ? "active" : ""} ${run?.status === "crashed" ? "crashed" : ""}`}
            tabIndex={0}
            data-proc={key}
            data-testid="process-row"
            data-status={run?.status ?? "stopped"}
            onClick={() => openProcess(key)}
            onKeyDown={(e) => {
              if (e.key === "Enter") openProcess(key);
            }}
            onContextMenu={(e) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY, defId: d.id }); }}
            title={`${d.command}${d.cwd && d.cwd !== "." ? `  (in ${d.cwd})` : ""}`}
          >
            <button
              className="proc-play"
              aria-label={running ? `Stop ${defLabel(d)}` : `Start ${defLabel(d)}`}
              title={running ? "Stop (⌘.)" : "Start"}
              onClick={(e) => { e.stopPropagation(); void (running ? stopProcess(p.id, d.id) : startProcess(p.id, d.id)); }}
            >
              {running ? "■" : "▶"}
            </button>
            <span className="proc-name">{defLabel(d)}{run?.baseLabel && isRunning(run) && <span className="muted" data-testid="proc-base"> · {run.baseLabel}</span>}</span>
            {here && isRunning(run) && run?.sessionId !== here.id && (
              <button className="link small proc-move" title={`Restart in ${here.glyph}'s worktree`} data-testid="proc-move" onClick={(e) => { e.stopPropagation(); void restartProcess(p.id, d.id, here.id); }}>↻ in {here.glyph}</button>
            )}
            <span className="proc-status" style={{ color: label.color }}>
              {label.symbol}{" "}
              {run?.status === "running" && run.detectedPorts.length ? (
                <>
                  {run.detectedPorts.map((port) => (
                    <a key={port} className="port-link" href={`http://localhost:${port}`}
                      onClick={(e) => { e.preventDefault(); e.stopPropagation(); void backend().then((b) => b.openUrl(`http://localhost:${port}`)); }}>
                      :{port}
                    </a>
                  ))}
                  <span className="muted"> · {label.text.split(" · ").pop()}</span>
                </>
              ) : (
                label.text
              )}
            </span>
            <span className="proc-hover">
              <button title="Restart (⌘R)" aria-label="Restart" onClick={(e) => { e.stopPropagation(); void restartProcess(p.id, d.id); }}>↻</button>
              <button title="Open in center pane" aria-label="Open" onClick={(e) => { e.stopPropagation(); openProcess(key); }}>⇱</button>
            </span>
          </div>
        );
      })}
      {sugg.length > 0 && (
        <div className="suggestions" data-testid="suggestions">
          <span className="muted small">Found in this project:</span>
          {sugg.map((d, i) => (
            <button key={d.id} className="sugg" onClick={() => void addSuggestion(i)} title={`Add “${d.command}” (doesn't start it)`}>
              + {d.command}
            </button>
          ))}
          <button className="sugg dismiss" onClick={() => void invoke("set_processes_offered", { projectId: p.id })}>dismiss</button>
        </div>
      )}
      <button className="ghost-row small" onClick={() => addOrEdit()}>+ Add process{synced ? " (fleet.yml)" : ""}</button>
      {menu && (() => {
        const d = p.processes.find((x) => x.id === menu.defId)!;
        const run = runs[procKey(p.id, d.id)];
        return (
          <ContextMenu
            x={menu.x}
            y={menu.y}
            onClose={() => setMenu(null)}
            items={[
              isRunning(run) ? { label: "Stop", onClick: () => void stopProcess(p.id, d.id) } : { label: "Start", onClick: () => void startProcess(p.id, d.id) },
              { label: "Restart", onClick: () => void restartProcess(p.id, d.id) },
              { label: "Open terminal", onClick: () => openProcess(procKey(p.id, d.id)) },
              { label: synced ? "Edit in fleet.yml…" : "Edit…", onClick: () => addOrEdit(d) },
              ...(synced ? [] : [{ label: "Delete", danger: true, onClick: () => void invoke("delete_process", { projectId: p.id, defId: d.id }) }]),
            ]}
          />
        );
      })()}
    </>
  );
}

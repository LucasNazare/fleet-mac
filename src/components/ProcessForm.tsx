import { useState } from "react";
import { useFleet } from "../store/store";
import { invoke } from "../ipc";
import type { ProcessDef } from "../types";

function envToText(env: Record<string, string>) {
  return Object.entries(env).map(([k, v]) => `${k}=${v}`).join("\n");
}
function textToEnv(t: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of t.split("\n")) {
    const i = line.indexOf("=");
    if (i > 0) out[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return out;
}

export function ProcessForm() {
  const { procForm, projects, setUi, showToast } = useFleet();
  const project = projects.find((p) => p.id === procForm?.projectId);
  const d = procForm?.def;
  const [name, setName] = useState(d?.name ?? "");
  const [command, setCommand] = useState(d?.command ?? "");
  const [cwd, setCwd] = useState(d?.cwd ?? ".");
  const [env, setEnv] = useState(envToText(d?.env ?? {}));
  const [autoStart, setAutoStart] = useState(d?.autoStart ?? false);
  const [autoRestart, setAutoRestart] = useState(d?.autoRestart ?? false);
  const [keepLocal, setKeepLocal] = useState(false);
  if (!procForm || !project) return null;

  const close = () => setUi({ procForm: null });
  const save = async () => {
    const def: ProcessDef = { id: d?.id ?? "", name: name.trim(), command: command.trim(), cwd: cwd.trim() || ".", env: textToEnv(env), autoStart, autoRestart, port: d?.port ?? null };
    try {
      await invoke("save_process", { projectId: project.id, def, keepLocal });
      close();
    } catch (e) {
      showToast(String(e), "error");
    }
  };

  return (
    <div className="overlay" onMouseDown={close}>
      <div className="sheet" role="dialog" aria-label="Process" onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") close();
          if (e.key === "Enter" && e.metaKey) void save();
        }}>
        <h3>{d ? "Edit process" : "Add process"} · {project.name}</h3>
        <label>Command
          <input className="field" autoFocus value={command} onChange={(e) => setCommand(e.target.value)} placeholder="npm run dev" data-testid="proc-command" />
        </label>
        <label>Name <span className="muted">(optional)</span>
          <input className="field" value={name} onChange={(e) => setName(e.target.value)} placeholder={command || "web"} />
        </label>
        <label>Working folder <span className="muted">(relative to the project)</span>
          <input className="field" value={cwd} onChange={(e) => setCwd(e.target.value)} placeholder="." />
        </label>
        <label>Environment <span className="muted">(KEY=value per line)</span>
          <textarea rows={2} value={env} onChange={(e) => setEnv(e.target.value)} placeholder="PORT=5173" />
        </label>
        <label className="check"><input type="checkbox" checked={autoStart} onChange={(e) => setAutoStart(e.target.checked)} /> Start automatically when fleet opens the project</label>
        <label className="check"><input type="checkbox" checked={autoRestart} onChange={(e) => setAutoRestart(e.target.checked)} /> Restart on crash (1 s, 2 s, 4 s … up to 30 s)</label>
        <label className="check"><input type="checkbox" checked={keepLocal} onChange={(e) => setKeepLocal(e.target.checked)} /> Keep .fleet/project.json out of git (adds it to .git/info/exclude)</label>
        <p className="muted small">Saved in {project.rootPath}/.fleet/project.json so the team can commit it.</p>
        <div className="sheet-actions">
          <button className="btn" onClick={close}>Cancel</button>
          <button className="btn primary" disabled={!command.trim()} onClick={() => void save()} data-testid="proc-save">Save ⌘↵</button>
        </div>
      </div>
    </div>
  );
}

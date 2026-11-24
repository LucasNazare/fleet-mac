import type { ProcessDef, ProcessRun } from "../types";
import { colors, duration } from "../theme";
import { backend } from "../ipc";

export interface RunLabel {
  symbol: string;
  text: string;
  color: string;
}

/** "● :5173 · up 2h", "✕ crashed (exit 1)", "■ stopped". */
export function runLabel(run: ProcessRun | undefined, now: number): RunLabel {
  if (!run) return { symbol: "■", text: "stopped", color: colors.muted };
  switch (run.status) {
    case "running": {
      if (run.stopping) return { symbol: "◌", text: "stopping…", color: colors.muted };
      const up = `up ${duration(now - run.startedAt)}`;
      const port = run.detectedPorts.length ? `:${run.detectedPorts.join(" :")} · ` : "";
      return { symbol: "●", text: `${port}${run.detached ? "detached · " : ""}${up}`, color: colors.green };
    }
    case "crashed": {
      const again = run.nextRestartAt ? ` · restarting in ${Math.max(0, Math.ceil((run.nextRestartAt - now) / 1000))}s` : "";
      return { symbol: "✕", text: `crashed (exit ${run.exitCode ?? "?"})${again}`, color: colors.red };
    }
    case "exited":
      return { symbol: "○", text: `exited (${run.exitCode ?? 0})`, color: colors.muted };
    default:
      return { symbol: "■", text: "stopped", color: colors.muted };
  }
}

export function PortLinks({ run }: { run?: ProcessRun }) {
  if (!run || run.status !== "running" || !run.detectedPorts.length) return null;
  return (
    <>
      {run.detectedPorts.map((p) => (
        <button
          key={p}
          className="port-link"
          title={`Open http://localhost:${p}`}
          onClick={(e) => {
            e.stopPropagation();
            void backend().then((b) => b.openUrl(`http://localhost:${p}`));
          }}
        >
          :{p}
        </button>
      ))}
    </>
  );
}

export const isRunning = (r?: ProcessRun) => r?.status === "running" && !r.stopping;

export function defLabel(d: ProcessDef) {
  return d.name || d.command;
}

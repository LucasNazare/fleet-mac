import { useFleet } from "../store/store";
import { invoke } from "../ipc";

export function QuitDialog() {
  const n = useFleet((s) => s.quitPrompt);
  if (n == null) return null;
  const close = () => useFleet.setState({ quitPrompt: null });
  return (
    <div className="overlay">
      <div className="sheet small" role="dialog" aria-label="Quit fleet" onKeyDown={(e) => { if (e.key === "Escape") close(); }}>
        <h3>Quit fleet?</h3>
        <p className="muted">
          {n} process{n === 1 ? " is" : "es are"} still running. Stopping sends Ctrl-C, then TERM and KILL, to each process group so no port stays busy.
        </p>
        <div className="sheet-actions">
          <button className="btn" onClick={close}>Cancel</button>
          <button className="btn" onClick={() => void invoke("quit_app", { stopProcesses: false })}>Leave running</button>
          <button className="btn primary" autoFocus onClick={() => void invoke("quit_app", { stopProcesses: true })}>Stop all and quit</button>
        </div>
      </div>
    </div>
  );
}

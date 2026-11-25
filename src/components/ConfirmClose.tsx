import { useFleet } from "../store/store";
import { invoke } from "../ipc";
import * as pool from "../terminal/termPool";
import { useSync } from "../store/sync";

export function ConfirmClose() {
  const confirm = useFleet((s) => s.confirm);
  const synced = useSync((st) => !!confirm && !!st.configs[confirm.session.projectId]?.enabled);
  if (!confirm) return null;
  const s = confirm.session;
  const done = () => useFleet.setState({ confirm: null });
  const act = async (archive: boolean, wt?: "keep" | "merge" | "delete") => {
    if (wt && wt !== "keep") {
      try {
        await invoke("worktree_finish", { sessionId: s.id, action: wt });
      } catch (e) {
        useFleet.getState().showToast(String(e), "error");
        return;
      }
    }
    await invoke("close_session", { id: s.id, archive });
    if (!archive) pool.dispose(s.id);
    done();
  };
  return (
    <div className="overlay" onMouseDown={done}>
      <div className="sheet small" onMouseDown={(e) => e.stopPropagation()} role="dialog"
        onKeyDown={(e) => { if (e.key === "Escape") done(); }}>
        <h3>Close “{s.title}”?</h3>
        <p className="muted">
          {s.state === "working" ? "This session is working right now. Closing stops claude mid-task. " : ""}
          Archive keeps it so you can resume it later with claude --resume.
        </p>
        {s.worktreePath ? (
          <>
            <p className="muted small">
              It runs in its own worktree ({s.branch || "fleet/…"}).{" "}
              {synced
                ? "This project syncs with GitHub: its work is already pushed, and it reaches main through Ship."
                : "What should happen to it?"}
            </p>
            <div className="sheet-actions wrap">
              <button className="btn" onClick={done} autoFocus>Cancel</button>
              <button className="btn" onClick={() => void act(true, "keep")}>Archive, keep worktree</button>
              <button className="btn" onClick={() => void act(false, "keep")}>Close, keep worktree</button>
              {synced ? (
                <button className="btn primary" onClick={() => { done(); useSync.setState({ ship: s.projectId }); }}>Ship to main… (⌘⇧D)</button>
              ) : (
                <button className="btn primary" onClick={() => void act(false, "merge")}>Merge into main checkout & remove</button>
              )}
              <button className="btn danger" onClick={() => void act(false, "delete")}>Delete worktree & branch</button>
            </div>
          </>
        ) : (
          <div className="sheet-actions">
            <button className="btn" onClick={done} autoFocus>Cancel</button>
            <button className="btn" onClick={() => void act(true)}>Archive</button>
            <button className="btn danger" onClick={() => void act(false)}>Close and forget</button>
          </div>
        )}
      </div>
    </div>
  );
}

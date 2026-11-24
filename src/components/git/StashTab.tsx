import { useState } from "react";
import { useGit } from "../../store/git";
import { invoke } from "../../ipc";
import { confirmDialog } from "../Dialogs";
import { relTime } from "./util";

export function StashTab({ root }: { root: string }) {
  const { repos, act } = useGit();
  const [msg, setMsg] = useState("");
  const stashes = repos[root]?.stashes;
  const run = (label: string, args: string[]) => act(root, label, () => invoke("git_run", { root, args }));
  return (
    <div className="stash">
      <div className="stash-new">
        <input className="field" placeholder="Message (optional)" value={msg} onChange={(e) => setMsg(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
        <button className="btn small" onClick={() => { void run("stash", ["stash", "push", "-u", ...(msg.trim() ? ["-m", msg.trim()] : [])]); setMsg(""); }} data-testid="stash-btn">Stash changes</button>
      </div>
      {!stashes ? <div className="muted pad">Loading…</div> : stashes.length === 0 ? <div className="muted pad">No stashes.</div> : stashes.map((s) => (
        <div key={s.name} className="branch-row">
          <span className="bname">{s.message}</span>
          <span className="spacer" />
          <span className="muted small">{relTime(s.time)}</span>
          <button className="link" onClick={() => void run("stash", ["stash", "apply", s.name])}>apply</button>
          <button className="link" onClick={() => void run("stash", ["stash", "pop", s.name])}>pop</button>
          <button className="link danger" onClick={() => confirmDialog({ title: `Drop ${s.name}?`, body: s.message, confirmLabel: "Drop", danger: true, onConfirm: () => void run("stash", ["stash", "drop", s.name]) })}>drop</button>
        </div>
      ))}
    </div>
  );
}

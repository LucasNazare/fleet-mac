import { useEffect, useState } from "react";
import { useFleet, projectSessions } from "../store/store";
import { invoke } from "../ipc";
import { Chip, StateLabel, useNow } from "./common";
import * as pool from "../terminal/termPool";

/** Open the "Send to session…" picker for the selection in terminal `sourceId`. */
export function openSendPicker(sourceId: string) {
  const st = useFleet.getState();
  const text = (pool.hasSelection(sourceId) ?? window.getSelection()?.toString() ?? "").trim();
  if (!text) {
    st.showToast("Select some text in the terminal first.");
    return;
  }
  let header = "";
  let projectId: string | null = null;
  if (sourceId.startsWith("proc:")) {
    const [, pid, did] = sourceId.split(":");
    const p = st.projects.find((x) => x.id === pid);
    const d = p?.processes.find((x) => x.id === did);
    header = `From ${d?.name ?? "a process"} in ${p?.name ?? "this project"}:`;
    projectId = pid;
  } else {
    const s = st.sessions[sourceId];
    header = s ? `From ${s.glyph} · ${s.title}:` : "From another session:";
    projectId = s?.projectId ?? null;
  }
  useFleet.setState({ sendPicker: { text, header, projectId } });
}

export function SendPicker() {
  const { sendPicker, sessions, projects, select } = useFleet();
  const now = useNow();
  const [i, setI] = useState(0);
  useEffect(() => setI(0), [sendPicker]);
  if (!sendPicker) return null;

  const pid = sendPicker.projectId;
  const list = (pid ? projectSessions(pid, sessions) : Object.values(sessions).filter((s) => !s.archived))
    .filter((s) => s.alive)
    .sort((a, b) => b.createdAt - a.createdAt);
  const close = () => useFleet.setState({ sendPicker: null });
  const send = async (id: string) => {
    const text = `${sendPicker.header}\n${sendPicker.text}\n`;
    await invoke("send_to_session", { sessionId: id, text }).catch((e) => useFleet.getState().showToast(String(e), "error"));
    close();
    select(id);
  };
  const projectName = pid ? projects.find((p) => p.id === pid)?.name : null;

  return (
    <div className="overlay" onMouseDown={close}>
      <div
        className="sheet"
        role="dialog"
        aria-label="Send to session"
        onMouseDown={(e) => e.stopPropagation()}
        tabIndex={-1}
        ref={(el) => el?.focus()}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") close();
          if (e.key === "ArrowDown") setI((x) => Math.min(list.length - 1, x + 1));
          if (e.key === "ArrowUp") setI((x) => Math.max(0, x - 1));
          if (e.key === "Enter" && list[i]) void send(list[i].id);
        }}
      >
        <h3>Send to a session{projectName ? ` in ${projectName}` : ""}</h3>
        <pre className="send-preview">{sendPicker.header}{"\n"}{sendPicker.text.length > 600 ? sendPicker.text.slice(0, 600) + "…" : sendPicker.text}</pre>
        <p className="muted small">Pasted into the prompt without submitting, so you can add to it.</p>
        {list.length === 0 && <p className="muted">No live sessions in this project.</p>}
        <div className="pick-list">
          {list.map((s, j) => (
            <button key={s.id} className={`pick ${j === i ? "on" : ""}`} onClick={() => void send(s.id)} onMouseEnter={() => setI(j)} data-testid="send-target">
              <Chip s={s} />
              <span className="pick-title">{s.title}</span>
              <StateLabel s={s} now={now} withDetail={false} />
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

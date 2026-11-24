import { memo, useState } from "react";
import type { Session } from "../types";
import { useFleet } from "../store/store";
import { Chip, StateLabel } from "./common";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { invoke } from "../ipc";
import { PALETTE } from "../theme";
import * as pool from "../terminal/termPool";
import { useSettings } from "../store/settings";
import { SyncBadge } from "./SyncUi";

export function sessionActions(s: Session, startRename: () => void, startGlyph: () => void): MenuItem[] {
  const st = useFleet.getState();
  const restart = async () => {
    const { cols, rows } = pool.currentSize();
    pool.reset(s.id);
    await invoke("restart_session", { id: s.id, cols, rows }).catch((e) => st.showToast(String(e), "error"));
    await pool.connect(s.id);
    st.select(s.id);
  };
  if (s.archived) {
    return [
      { label: "Resume", onClick: restart },
      { label: "Delete permanently", danger: true, onClick: () => void invoke("close_session", { id: s.id, archive: false }) },
    ];
  }
  return [
    { label: "Rename…", onClick: startRename },
    { label: "Pin glyph…", onClick: startGlyph },
    { label: "Pin color", swatches: PALETTE.map((c) => ({ color: c, onPick: () => void invoke("pin_identity", { id: s.id, color: c }) })) },
    { label: "Duplicate (new session here)", onClick: () => void useFleet.getState().quickSession(s.projectId) },
    { label: "Restart (claude --resume)", onClick: restart },
    { label: "Interrupt (Esc)", disabled: !s.alive, onClick: () => void invoke("pty_write", { id: s.id, data: "\x1b" }) },
    (() => {
      const muted = useSettings.getState().settings.notifications.mutedSessions.includes(s.id);
      return { label: muted ? "Unmute notifications" : "Mute notifications", onClick: () => useSettings.getState().update((x) => {
        x.notifications.mutedSessions = muted ? x.notifications.mutedSessions.filter((i) => i !== s.id) : [...x.notifications.mutedSessions, s.id];
      }) };
    })(),
    { label: "Archive", onClick: () => void invoke("close_session", { id: s.id, archive: true }) },
    { label: "Close…", danger: true, onClick: () => confirmClose(s) },
  ];
}

export function confirmClose(s: Session) {
  useFleet.setState({ confirm: { session: s } });
}

export const SessionRow = memo(function SessionRow({ s, now }: { s: Session; now: number }) {
  const active = useFleet((st) => st.activeSessionId === s.id || st.splitSessionId === s.id);
  const select = useFleet((st) => st.select);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [editing, setEditing] = useState<null | "title" | "glyph">(null);
  const [draft, setDraft] = useState("");

  const commit = async () => {
    if (editing === "title" && draft.trim() !== s.title) await invoke("rename_session", { id: s.id, title: draft });
    if (editing === "glyph" && draft.trim().length === 2) await invoke("pin_identity", { id: s.id, glyph: draft });
    setEditing(null);
  };

  return (
    <div
      className={`session-row ${active ? "active" : ""} ${s.archived ? "archived" : ""}`}
      style={active ? { boxShadow: `inset 3px 0 0 ${s.color}` } : undefined}
      onClick={() => (s.archived ? null : select(s.id))}
      onContextMenu={(e) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY }); }}
      data-testid="session-row"
      data-state={s.state}
      title={s.titleHistory.length ? `was: ${s.titleHistory[s.titleHistory.length - 1]}` : undefined}
    >
      <Chip s={s} />
      <div className="row-text">
        {editing ? (
          <input
            autoFocus
            className="inline-input"
            value={draft}
            maxLength={editing === "glyph" ? 2 : 80}
            placeholder={editing === "title" ? "Empty = auto titles" : "2 letters"}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter") void commit();
              if (e.key === "Escape") setEditing(null);
            }}
            onBlur={() => void commit()}
            onClick={(e) => e.stopPropagation()}
          />
        ) : (
          <div className="row-title" onDoubleClick={(e) => { e.stopPropagation(); setDraft(s.title); setEditing("title"); }}>
            {s.title}
            {s.titleLocked && <span className="muted small"> ·pinned</span>}
            {s.remote && <span className="remote-badge small" title="Works on a server over SSH"> ⇄</span>}
          </div>
        )}
        <div className="row-state">
          {s.archived ? <span className="muted">archived · right-click to resume</span> : <><StateLabel key={s.state} s={s} now={now} />{s.sync && <> <SyncBadge s={s} /></>}</>}
        </div>
      </div>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={sessionActions(s, () => { setDraft(s.title); setEditing("title"); }, () => { setDraft(s.glyph); setEditing("glyph"); })}
        />
      )}
    </div>
  );
});

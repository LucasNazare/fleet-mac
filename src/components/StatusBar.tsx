import { memo } from "react";
import { useFleet, type Filter } from "../store/store";
import { STATE } from "../theme";
import { SyncStatusItem } from "./SyncUi";
import { useNow } from "./common";
import { UsageToday } from "./Usage";

export const StatusBar = memo(function StatusBar() {
  const { sessions, activeSessionId, filter, setUi } = useFleet();
  const live = Object.values(sessions).filter((s) => !s.archived);
  const count = (g: Filter) => live.filter((s) => STATE[s.state].group === g).length;
  const items: { f: Filter; sym: string; label: string; color: string }[] = [
    { f: "needs", sym: "●", label: "need you", color: STATE.needs_reply.color },
    { f: "working", sym: "◐", label: "working", color: STATE.working.color },
    { f: "done", sym: "✓", label: "to review", color: STATE.done.color },
    { f: "failed", sym: "✕", label: "failed", color: STATE.failed.color },
    { f: "idle", sym: "○", label: "idle", color: STATE.idle.color },
  ];
  const active = activeSessionId ? sessions[activeSessionId] : null;
  const now = useNow(10000);
  return (
    <footer className="statusbar">
      {items.map((it) => {
        const n = count(it.f);
        if (n === 0 && it.f === "failed") return null;
        return (
          <button
            key={it.f}
            className={`status-count ${filter === it.f ? "on" : ""}`}
            style={{ color: n ? it.color : undefined }}
            onClick={() => setUi({ filter: filter === it.f ? null : it.f })}
            data-testid={`count-${it.f}`}
            title={`Show only sessions that are ${it.label}`}
          >
            {it.sym} {n} {it.label}
          </button>
        );
      })}
      <span className="spacer" />
      <UsageToday />
      <SyncStatusItem now={now} />
      {active?.branch && <span className="muted">⎇ {active.branch}</span>}
      <span className="muted hints">
        <kbd>⌘]</kbd>/<kbd>⌘[</kbd> next/prev <kbd>⌘B</kbd> sidebar <kbd>⌘G</kbd> git
      </span>
    </footer>
  );
});

import { useEffect, useState } from "react";
import type { Session } from "../types";
import { STATE, duration } from "../theme";
import { readableOn } from "../themes";

/** Re-render every `ms` so "working · 11m" stays current. */
export function useNow(ms = 15000) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), ms);
    return () => window.clearInterval(t);
  }, [ms]);
  return now;
}

export function Chip({ s, size = 22 }: { s: Pick<Session, "glyph" | "color">; size?: number }) {
  return (
    <span
      className="chip"
      style={{ background: s.color, color: readableOn(s.color), width: size, height: size, fontSize: Math.round(size * 0.44), borderRadius: Math.round(size * 0.22) }}
      aria-hidden
    >
      {s.glyph}
    </span>
  );
}

export function StateLabel({ s, now, withDetail = true }: { s: Session; now: number; withDetail?: boolean }) {
  const m = STATE[s.state];
  const detail = s.pendingQuestion
    ? s.pendingQuestion
    : s.state === "working" || s.state === "done" || s.state === "failed"
      ? duration(now - s.stateSince)
      : "";
  return (
    <span className="state-label" style={{ color: m.color }}>
      <span className="sym">{m.symbol}</span> {m.word}
      {withDetail && detail ? <span className="detail"> · {detail}</span> : null}
    </span>
  );
}

export function ContextMeter({ pct }: { pct?: number | null }) {
  if (pct == null) return <span className="ctx muted" title="Context use appears after the first reply">ctx —</span>;
  const warn = pct >= 80;
  return (
    <span className={`ctx ${warn ? "warn" : ""}`} title={warn ? `${pct.toFixed(0)}% of context used — consider /compact` : `${pct.toFixed(0)}% of context used`}>
      <span className="ctx-bar"><span style={{ width: `${Math.min(100, pct)}%` }} /></span>
      {pct.toFixed(0)}%
    </span>
  );
}

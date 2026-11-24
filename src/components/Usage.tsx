// Token spend and turn timing: the session header badge, the status bar's "today"
// total and the Usage sheet (per project, per day). Costs are estimates at API list
// prices — on a subscription they show relative spend, not a bill.
import { useEffect, useMemo, useState } from "react";
import { create } from "zustand";
import type { Session, Tally, UsageDay } from "../types";
import { useFleet } from "../store/store";
import { invoke } from "../ipc";

export const useUsage = create<{ open: boolean }>(() => ({ open: false }));
export const openUsage = () => useUsage.setState({ open: true });

export function fmtCost(usd: number) {
  if (usd <= 0) return "$0";
  if (usd < 0.01) return "<$0.01";
  if (usd < 100) return `$${usd.toFixed(2)}`;
  return `$${Math.round(usd).toLocaleString()}`;
}

export function fmtTokens(n: number) {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${Math.round(n / 1e3)}k`;
  return String(n);
}

export function fmtDur(ms: number) {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

const tokens = (t: Tally) => t.input + t.output + t.cacheRead + t.cacheWrite;
const pct = (part: number, whole: number) => (whole > 0 ? Math.round((part / whole) * 100) : 0);

function tallyLine(label: string, t: Tally) {
  const extra = t.unpriced ? ` (+${fmtTokens(t.unpriced)} from unpriced models)` : "";
  return `${label}: ${fmtCost(t.cost)} · ${fmtTokens(tokens(t))} tokens — ${fmtTokens(t.input)} in, ${fmtTokens(t.output)} out, ${fmtTokens(t.cacheRead)} cache read, ${fmtTokens(t.cacheWrite)} cache write${extra}`;
}

/** Cost and turn time in the session header. */
export function SessionUsage({ s, now }: { s: Session; now: number }) {
  const m = s.metrics;
  if (!m) return null;
  const cost = m.main.cost + m.sub.cost;
  const total = tokens(m.main) + tokens(m.sub);
  const subShare = cost > 0 ? pct(m.sub.cost, cost) : pct(tokens(m.sub), total);
  const running = m.turnStarted != null;
  const costTip = [
    tallyLine("Main thread", m.main),
    tallyLine("Sub-agents", m.sub),
    total ? `Sub-agents: ${subShare}% of this session` : "",
    "Estimated at Anthropic API list prices.",
  ].filter(Boolean).join("\n");
  const avg = m.turns ? m.turnMs / m.turns : 0;
  const turnTip = m.turns
    ? `${m.turns} turn${m.turns === 1 ? "" : "s"}, avg ${fmtDur(avg)}, total ${fmtDur(m.turnMs)}` +
      (m.waitMs ? `\n${fmtDur(m.waitMs)} of it waiting on you (${pct(m.waitMs, m.turnMs)}%)` : "") +
      (m.lastTurnMs != null ? `\nLast turn ${fmtDur(m.lastTurnMs)}${m.lastWaitMs ? `, ${fmtDur(m.lastWaitMs)} waiting on you` : ""}` : "")
    : "Time from your prompt to Claude's reply";
  return (
    <span className="usage-hdr" data-testid="session-usage">
      {(total > 0 || cost > 0) && (
        <span className="usage-cost" title={costTip} data-testid="session-cost">
          {fmtCost(cost)}
          {subShare > 0 && <span className="muted"> · {subShare}% agents</span>}
        </span>
      )}
      {running ? (
        <span className="usage-turn live" title={turnTip} data-testid="turn-time">◷ {fmtDur(now - m.turnStarted!)}</span>
      ) : m.lastTurnMs != null ? (
        <span className="usage-turn muted" title={turnTip} data-testid="turn-time">◷ {fmtDur(m.lastTurnMs)}</span>
      ) : null}
    </span>
  );
}

/** Sum of all sessions' cost; changes whenever new usage is counted. */
const useSpendKey = () => useFleet((st) => Object.values(st.sessions).reduce((a, s) => a + (s.metrics ? s.metrics.main.cost + s.metrics.sub.cost : 0), 0));

function useReport(days: number, refreshMs: number) {
  const key = useSpendKey();
  const [rows, setRows] = useState<UsageDay[] | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () => invoke<UsageDay[]>("usage_report", { days }).then((r) => alive && setRows(r)).catch(() => alive && setRows([]));
    void load();
    const t = window.setInterval(load, refreshMs);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [days, refreshMs, key]);
  return rows;
}

/** Today's estimated spend in the status bar; opens the Usage sheet. */
export function UsageToday() {
  const rows = useReport(1, 30_000);
  const cost = (rows ?? []).reduce((a, r) => a + r.cost + r.subCost, 0);
  if (!rows || cost <= 0) return null;
  return (
    <button className="status-count muted" onClick={openUsage} title="Estimated spend today (API list prices) — click for usage by project" data-testid="usage-today">
      {fmtCost(cost)} today
    </button>
  );
}

interface ProjectRow {
  project: string;
  name: string;
  cost: number;
  subCost: number;
  tokens: number;
  turns: number;
  turnMs: number;
  waitMs: number;
  maxTurnMs: number;
}

export function UsageSheet() {
  const open = useUsage((u) => u.open);
  return open ? <UsageSheetBody /> : null;
}

function UsageSheetBody() {
  const [days, setDays] = useState(7);
  const rows = useReport(days, 60_000);
  const projects = useFleet((st) => st.projects);
  const close = () => useUsage.setState({ open: false });

  const { byProject, byDay, total } = useMemo(() => {
    const name = (id: string) => projects.find((p) => p.id === id)?.name ?? "Removed project";
    const bp = new Map<string, ProjectRow>();
    const bd = new Map<string, { day: string; cost: number; subCost: number; turns: number; turnMs: number }>();
    for (const r of rows ?? []) {
      const p = bp.get(r.project) ?? { project: r.project, name: name(r.project), cost: 0, subCost: 0, tokens: 0, turns: 0, turnMs: 0, waitMs: 0, maxTurnMs: 0 };
      p.cost += r.cost; p.subCost += r.subCost; p.tokens += r.tokens + r.subTokens;
      p.turns += r.turns; p.turnMs += r.turnMs; p.waitMs += r.waitMs; p.maxTurnMs = Math.max(p.maxTurnMs, r.maxTurnMs);
      bp.set(r.project, p);
      const d = bd.get(r.day) ?? { day: r.day, cost: 0, subCost: 0, turns: 0, turnMs: 0 };
      d.cost += r.cost; d.subCost += r.subCost; d.turns += r.turns; d.turnMs += r.turnMs;
      bd.set(r.day, d);
    }
    const byProject = [...bp.values()].sort((a, b) => b.cost + b.subCost - (a.cost + a.subCost));
    const byDay = [...bd.values()].sort((a, b) => b.day.localeCompare(a.day));
    const total = byProject.reduce((a, p) => a + p.cost + p.subCost, 0);
    return { byProject, byDay, total };
  }, [rows, projects]);

  return (
    <div className="overlay" onMouseDown={close}>
      <div className="sheet usage-sheet" role="dialog" aria-label="Usage" onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") close(); }} tabIndex={-1} data-testid="usage-sheet">
        <div className="usage-top">
          <h3>Usage</h3>
          <span className="spacer" />
          {[1, 7, 30].map((d) => (
            <button key={d} className={`btn small ${days === d ? "primary" : ""}`} onClick={() => setDays(d)} aria-pressed={days === d}>
              {d === 1 ? "Today" : `${d} days`}
            </button>
          ))}
        </div>
        <div className="usage-total">
          <b data-testid="usage-total">{fmtCost(total)}</b>
          <span className="muted"> estimated at API list prices{days === 1 ? " today" : ` over ${days} days`}</span>
        </div>
        {rows === null ? (
          <p className="muted">Loading…</p>
        ) : byProject.length === 0 ? (
          <p className="muted">No usage recorded in this period yet. Spend is recorded as sessions reply.</p>
        ) : (
          <>
            <table className="usage-table" data-testid="usage-projects">
              <thead>
                <tr><th>Project</th><th className="num">Cost</th><th className="num">Sub-agents</th><th className="num">Tokens</th><th className="num">Turns</th><th className="num">Avg turn</th><th className="num">Longest</th><th className="num">Waiting on you</th></tr>
              </thead>
              <tbody>
                {byProject.map((p) => {
                  const c = p.cost + p.subCost;
                  return (
                    <tr key={p.project}>
                      <td>{p.name}</td>
                      <td className="num">{fmtCost(c)}</td>
                      <td className="num muted">{c > 0 ? `${pct(p.subCost, c)}%` : "—"}</td>
                      <td className="num muted">{fmtTokens(p.tokens)}</td>
                      <td className="num">{p.turns || "—"}</td>
                      <td className="num">{p.turns ? fmtDur(p.turnMs / p.turns) : "—"}</td>
                      <td className="num muted">{p.turns ? fmtDur(p.maxTurnMs) : "—"}</td>
                      <td className="num muted">{p.turnMs ? `${pct(p.waitMs, p.turnMs)}%` : "—"}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            {days > 1 && (
              <table className="usage-table" data-testid="usage-days">
                <thead><tr><th>Day</th><th className="num">Cost</th><th className="num">Sub-agents</th><th className="num">Turns</th><th className="num">Avg turn</th></tr></thead>
                <tbody>
                  {byDay.map((d) => (
                    <tr key={d.day}>
                      <td>{d.day}</td>
                      <td className="num">{fmtCost(d.cost + d.subCost)}</td>
                      <td className="num muted">{d.cost + d.subCost > 0 ? `${pct(d.subCost, d.cost + d.subCost)}%` : "—"}</td>
                      <td className="num">{d.turns || "—"}</td>
                      <td className="num">{d.turns ? fmtDur(d.turnMs / d.turns) : "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </>
        )}
        <p className="muted small usage-note">
          Turn = your prompt until Claude stops; “waiting on you” is the part spent on permission prompts and questions.
          Cost counts input, output and cache tokens at Anthropic API list prices, so it estimates spend on a subscription rather than billing it.
        </p>
      </div>
    </div>
  );
}

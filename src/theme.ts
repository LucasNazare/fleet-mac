import type { SessionState } from "./types";

/** Semantic colors as CSS variables: every theme (src/themes) defines them. */
export const colors = {
  bg: "var(--bg)",
  panel: "var(--panel)",
  border: "var(--border)",
  text: "var(--text)",
  muted: "var(--muted)",
  amber: "var(--state-needs)",
  blue: "var(--state-working)",
  green: "var(--state-done)",
  red: "var(--state-failed)",
  grey: "var(--state-idle)",
};

export const PALETTE = [
  "#FFA657", "#58A6FF", "#7EE787", "#D2A8FF", "#FF7B72", "#56D4DD",
  "#E3B341", "#DB61A2", "#3FB950", "#A5D6FF", "#FF9BCE", "#B4A0FF",
];

export interface StateMeta {
  symbol: string;
  word: string;
  color: string;
  group: "needs" | "working" | "done" | "failed" | "idle";
}

export const STATE: Record<SessionState, StateMeta> = {
  needs_permission: { symbol: "●", word: "needs you", color: colors.amber, group: "needs" },
  needs_reply: { symbol: "●", word: "needs you", color: colors.amber, group: "needs" },
  working: { symbol: "◐", word: "working", color: colors.blue, group: "working" },
  done: { symbol: "✓", word: "done", color: colors.green, group: "done" },
  failed: { symbol: "✕", word: "failed", color: colors.red, group: "failed" },
  idle: { symbol: "○", word: "idle", color: colors.grey, group: "idle" },
  queued: { symbol: "◌", word: "queued", color: colors.grey, group: "idle" },
};

export const needsYou = (s: SessionState) => s === "needs_permission" || s === "needs_reply";

export function duration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d`;
}

import type { FileEntry } from "../../types";

export function relTime(unixSec: number) {
  const s = Math.max(0, Date.now() / 1000 - unixSec);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d ago`;
  if (s < 86400 * 365) return `${Math.floor(s / 86400 / 30)}mo ago`;
  return `${Math.floor(s / 86400 / 365)}y ago`;
}

export const LETTER_COLOR: Record<string, string> = {
  M: "var(--state-needs)", A: "var(--state-done)", D: "var(--state-failed)", R: "var(--cyan)", C: "var(--cyan)", U: "var(--state-failed)", "?": "var(--state-done)", T: "var(--state-needs)",
};

export function letterFor(f: FileEntry, staged: boolean): string {
  if (f.conflicted) return "U";
  if (f.untracked) return "A";
  return staged ? f.index : f.worktree;
}

export function splitPath(p: string) {
  const i = p.lastIndexOf("/");
  return i < 0 ? { dir: "", name: p } : { dir: p.slice(0, i + 1), name: p.slice(i + 1) };
}

export function Counts({ a, r }: { a?: number | null; r?: number | null }) {
  if (a == null && r == null) return null;
  return (
    <span className="counts-pm">
      {a ? <span className="plus">+{a}</span> : null}
      {r ? <span className="minus">−{r}</span> : null}
    </span>
  );
}

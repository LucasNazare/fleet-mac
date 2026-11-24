// Diff gutter vs HEAD, with lines attributed to sessions in their colour.
import { diffLines } from "diff";
import { monaco } from "./monaco";
import type { Session } from "../types";

export interface LineChange {
  kind: "added" | "modified" | "removed";
  from: number;
  to: number;
}

/** Changed lines of `current` relative to `head` (1-based, inclusive). */
export function lineChanges(head: string, current: string): LineChange[] {
  const out: LineChange[] = [];
  let line = 1;
  const parts = diffLines(head, current);
  for (let i = 0; i < parts.length; i++) {
    const p = parts[i];
    const n = p.count ?? 0;
    if (p.removed) {
      const next = parts[i + 1];
      if (next?.added) {
        const m = next.count ?? 0;
        out.push({ kind: "modified", from: line, to: line + m - 1 });
        line += m;
        i++;
      } else {
        out.push({ kind: "removed", from: line, to: line });
      }
    } else if (p.added) {
      out.push({ kind: "added", from: line, to: line + n - 1 });
      line += n;
    } else {
      line += n;
    }
  }
  return out;
}

const styled = new Set<string>();
function sessionClass(s: Session) {
  const cls = `gs-${s.color.slice(1)}`;
  if (!styled.has(cls)) {
    styled.add(cls);
    const st = document.createElement("style");
    st.textContent = `.monaco-editor .${cls}{background:${s.color};width:4px!important;margin-left:3px;border-radius:2px}`;
    document.head.appendChild(st);
  }
  return cls;
}

export function gutterDecorations(changes: LineChange[], path: string, sessions: Session[], maxLine: number): monaco.editor.IModelDeltaDecoration[] {
  const decos: monaco.editor.IModelDeltaDecoration[] = [];
  const owners = sessions.filter((s) => s.lineMarks?.[path]?.length);
  for (const c of changes) {
    const from = Math.min(Math.max(1, c.from), maxLine);
    const to = Math.min(Math.max(from, c.to), maxLine);
    if (c.kind === "removed") {
      decos.push({ range: new monaco.Range(from, 1, from, 1), options: { linesDecorationsClassName: "gutter-removed" } });
      continue;
    }
    for (let l = from; l <= to; l++) {
      const owner = owners.find((s) => s.lineMarks![path].some(([a, b]) => l >= a && l <= b));
      decos.push({
        range: new monaco.Range(l, 1, l, 1),
        options: owner
          ? {
              linesDecorationsClassName: sessionClass(owner),
              glyphMarginHoverMessage: { value: `Changed by **${owner.glyph} · ${owner.title}**` },
              glyphMarginClassName: "gutter-owner-glyph",
            }
          : { linesDecorationsClassName: c.kind === "added" ? "gutter-added" : "gutter-modified" },
      });
    }
  }
  return decos;
}

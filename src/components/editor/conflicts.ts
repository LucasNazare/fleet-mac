import { monaco } from "../../editor/monaco";

export interface Conflict {
  start: number;
  base?: number;
  mid: number;
  end: number;
}

/** Conflict blocks from git's markers (1-based line numbers). */
export function parseConflicts(lines: string[]): Conflict[] {
  const out: Conflict[] = [];
  let cur: Partial<Conflict> | null = null;
  lines.forEach((l, i) => {
    const n = i + 1;
    if (l.startsWith("<<<<<<<")) cur = { start: n };
    else if (cur && l.startsWith("|||||||")) cur.base = n;
    else if (cur && l.startsWith("=======")) cur.mid = n;
    else if (cur && cur.mid && l.startsWith(">>>>>>>")) {
      cur.end = n;
      out.push(cur as Conflict);
      cur = null;
    }
  });
  return out;
}

export function resolveText(lines: string[], c: Conflict, take: "current" | "incoming" | "both"): string[] {
  const current = lines.slice(c.start, (c.base ?? c.mid) - 1);
  const incoming = lines.slice(c.mid, c.end - 1);
  return take === "current" ? current : take === "incoming" ? incoming : [...current, ...incoming];
}

/** Resolve the conflict at (or after) the cursor. */
export function resolveConflict(editor: monaco.editor.IStandaloneCodeEditor, take: "current" | "incoming" | "both") {
  const model = editor.getModel()!;
  const lines = model.getLinesContent();
  const cs = parseConflicts(lines);
  const line = editor.getPosition()?.lineNumber ?? 1;
  const c = cs.find((x) => line <= x.end) ?? cs[0];
  if (!c) return;
  const text = resolveText(lines, c, take).join("\n");
  const range = new monaco.Range(c.start, 1, c.end, model.getLineMaxColumn(c.end));
  model.pushEditOperations([], [{ range, text }], () => null);
  const next = parseConflicts(model.getLinesContent())[0];
  if (next) editor.revealLineInCenter(next.start);
  editor.setPosition({ lineNumber: next?.start ?? c.start, column: 1 });
}

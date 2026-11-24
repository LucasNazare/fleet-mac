// ⌘I: describe a change for the selection (or cursor line) and pick who makes it.
import { useEffect, useRef, useState } from "react";
import { monaco } from "../../editor/monaco";
import { invoke } from "../../ipc";
import { useFleet } from "../../store/store";
import { relPath } from "../../editor/active";
import type { Session } from "../../types";
import * as pool from "../../terminal/termPool";

interface Pending {
  range: monaco.Range;
  oldText: string;
  zoneId: string | null;
  decos: monaco.editor.IEditorDecorationsCollection;
}

export function owningSession(path: string): Session | null {
  const ss = Object.values(useFleet.getState().sessions).filter((s) => !s.archived && s.alive && s.filesTouched.includes(path));
  return ss.sort((a, b) => b.stateSince - a.stateSince)[0] ?? null;
}

export function InlineEdit({ editor, path, root, projectId, onClose }: { editor: monaco.editor.IStandaloneCodeEditor; path: string; root: string; projectId: string; onClose: () => void }) {
  const model = editor.getModel()!;
  const sel = editor.getSelection()!;
  const startLine = sel.startLineNumber;
  const endLine = sel.isEmpty() ? sel.startLineNumber : sel.endColumn === 1 && sel.endLineNumber > sel.startLineNumber ? sel.endLineNumber - 1 : sel.endLineNumber;
  const pos = editor.getScrolledVisiblePosition({ lineNumber: startLine, column: 1 });
  const [request, setRequest] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const owner = owningSession(path);
  const rel = relPath(root, path);
  const selectedText = model.getValueInRange(new monaco.Range(startLine, 1, endLine, model.getLineMaxColumn(endLine)));

  useEffect(() => input.current?.focus(), []);

  const finish = (accept: boolean) => {
    if (pending) {
      if (pending.zoneId) editor.changeViewZones((a) => a.removeZone(pending.zoneId!));
      pending.decos.clear();
      if (!accept) {
        const cur = model.getValueInRange(pending.range);
        void cur;
        model.pushEditOperations([], [{ range: pending.range, text: pending.oldText }], () => null);
      }
    }
    onClose();
    editor.focus();
  };

  // ⌘↵ accept / Esc reject while a result is shown.
  useEffect(() => {
    if (!pending) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Enter" && e.metaKey) { e.preventDefault(); e.stopPropagation(); finish(true); }
      if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); finish(false); }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  const quickEdit = async () => {
    if (!request.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const replacement = await invoke<string>("quick_edit", { root, path: rel, text: model.getValue(), start: startLine, end: endLine, request, model: null });
      const range = new monaco.Range(startLine, 1, endLine, model.getLineMaxColumn(endLine));
      model.pushEditOperations([], [{ range, text: replacement }], () => null);
      const newLines = replacement.split("\n").length;
      const newRange = new monaco.Range(startLine, 1, startLine + newLines - 1, model.getLineMaxColumn(startLine + newLines - 1));
      const decos = editor.createDecorationsCollection([{ range: newRange, options: { isWholeLine: true, className: "inline-added" } }]);
      let zoneId: string | null = null;
      editor.changeViewZones((a) => {
        const dom = document.createElement("div");
        dom.className = "inline-removed";
        dom.textContent = selectedText;
        zoneId = a.addZone({ afterLineNumber: startLine - 1, heightInLines: selectedText.split("\n").length, domNode: dom });
      });
      setPending({ range: newRange, oldText: selectedText, zoneId, decos });
    } catch (e) {
      setError(String(e));
    }
    setBusy(false);
  };

  const context = `${rel} lines ${startLine}–${endLine}`;
  const sendToOwner = async () => {
    if (!owner || !request.trim()) return;
    await invoke("send_to_session", { sessionId: owner.id, text: `In ${context}: ${request.trim()}\n` });
    useFleet.getState().select(owner.id);
    onClose();
  };
  const newSession = async () => {
    if (!request.trim()) return;
    const prompt = `In ${context}: ${request.trim()}\n\nCurrent code:\n\`\`\`\n${selectedText}\n\`\`\``;
    onClose();
    await useFleet.getState().newSession(projectId, prompt, "default", pool.currentSize());
  };

  const top = Math.max(4, (pos?.top ?? 0) - 6);
  return (
    <div className="inline-edit" style={{ top, left: 60 }} onMouseDown={(e) => e.stopPropagation()} data-testid="inline-edit">
      {pending ? (
        <div className="ie-row">
          <span>Review the change</span>
          <span className="spacer" />
          <button className="btn small primary" onClick={() => finish(true)} data-testid="ie-accept">Accept ⌘↵</button>
          <button className="btn small" onClick={() => finish(false)}>Reject Esc</button>
        </div>
      ) : (
        <>
          <textarea
            ref={input}
            rows={2}
            value={request}
            placeholder={`Change ${startLine === endLine ? `line ${startLine}` : `lines ${startLine}–${endLine}`}…`}
            onChange={(e) => setRequest(e.target.value)}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Escape") onClose();
              if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void quickEdit(); }
            }}
            data-testid="ie-input"
          />
          <div className="ie-row">
            <button className="btn small primary" disabled={busy || !request.trim()} onClick={() => void quickEdit()} data-testid="ie-quick">{busy ? "Editing…" : "Quick edit ↵"}</button>
            {owner && <button className="btn small" disabled={busy || !request.trim()} onClick={() => void sendToOwner()}>Send to {owner.glyph}</button>}
            <button className="btn small" disabled={busy || !request.trim()} onClick={() => void newSession()}>New session</button>
            <span className="spacer" />
            <span className="muted small">Esc</span>
          </div>
          {error && <div className="ie-error">{error}</div>}
        </>
      )}
    </div>
  );
}

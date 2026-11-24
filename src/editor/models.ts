// Monaco models for files on disk: load, dirty tracking, save (+format), reload.
import { monaco, languageFor } from "./monaco";
import { invoke } from "../ipc";
import { useEditor } from "../store/editor";

interface Doc {
  path: string;
  model: monaco.editor.ITextModel;
  savedVersion: number;
  /** Text as last loaded/saved — used to ignore our own writes in fs-changed. */
  diskText: string;
}

const docs = new Map<string, Doc>();

export interface FileText {
  text: string;
  mtime: number;
  binary: boolean;
  tooLarge: boolean;
}

export function getDoc(path: string) {
  return docs.get(path);
}

export function isDirty(path: string) {
  const d = docs.get(path);
  return !!d && d.model.getAlternativeVersionId() !== d.savedVersion;
}

function track(d: Doc) {
  d.model.onDidChangeContent(() => useEditor.getState().setDirty(d.path, isDirty(d.path)));
}

/** Load (or return the cached) model for an absolute path. */
export async function openDoc(path: string): Promise<Doc | { error: string }> {
  const existing = docs.get(path);
  if (existing && !existing.model.isDisposed()) return existing;
  let f: FileText;
  try {
    f = await invoke<FileText>("fs_read", { path });
  } catch (e) {
    return { error: String(e) };
  }
  if (f.binary) return { error: "This is a binary file." };
  if (f.tooLarge) return { error: "This file is larger than 8 MB." };
  const uri = monaco.Uri.file(path);
  const model = monaco.editor.getModel(uri) ?? monaco.editor.createModel(f.text, languageFor(path), uri);
  if (model.getValue() !== f.text) model.setValue(f.text);
  const d: Doc = { path, model, savedVersion: model.getAlternativeVersionId(), diskText: f.text };
  docs.set(path, d);
  track(d);
  return d;
}

/** Create a model for a path without opening a tab (LSP go-to-definition targets). */
export async function ensureModel(path: string) {
  if (monaco.editor.getModel(monaco.Uri.file(path))) return;
  await openDoc(path);
}

export async function saveDoc(path: string, opts: { root?: string; format?: boolean } = {}) {
  const d = docs.get(path);
  if (!d) return;
  let text = d.model.getValue();
  if (opts.format && opts.root) {
    try {
      const formatted = await invoke<string | null>("format_text", { root: opts.root, path, text });
      if (formatted != null && formatted !== text) {
        replaceKeepingView(d.model, formatted);
        text = formatted;
      }
    } catch (e) {
      console.warn("[fleet] format failed", e);
    }
  }
  await invoke("fs_write", { path, text });
  d.diskText = text;
  d.savedVersion = d.model.getAlternativeVersionId();
  useEditor.getState().setDirty(path, false);
}

/** Replace the whole text as one undoable edit (cursor/scroll are kept by the editor). */
export function replaceKeepingView(model: monaco.editor.ITextModel, text: string) {
  model.pushEditOperations([], [{ range: model.getFullModelRange(), text }], () => null);
}

/**
 * The file changed on disk. Returns "reloaded" (no local edits), "conflict"
 * (local unsaved edits; caller shows the Keep mine / Take theirs bar), or "same".
 */
export async function onDiskChange(path: string): Promise<"reloaded" | "conflict" | "same"> {
  const d = docs.get(path);
  if (!d) return "same";
  const f = await invoke<FileText>("fs_read", { path }).catch(() => null);
  if (!f || f.binary || f.tooLarge) return "same";
  if (f.text === d.diskText || f.text === d.model.getValue()) {
    d.diskText = f.text;
    return "same";
  }
  if (isDirty(path)) {
    pendingDisk.set(path, f.text);
    return "conflict";
  }
  replaceKeepingView(d.model, f.text);
  d.diskText = f.text;
  d.savedVersion = d.model.getAlternativeVersionId();
  useEditor.getState().setDirty(path, false);
  return "reloaded";
}

const pendingDisk = new Map<string, string>();
export function takeTheirs(path: string) {
  const d = docs.get(path);
  const text = pendingDisk.get(path);
  if (!d || text == null) return;
  replaceKeepingView(d.model, text);
  d.diskText = text;
  d.savedVersion = d.model.getAlternativeVersionId();
  pendingDisk.delete(path);
  useEditor.getState().setDirty(path, false);
}
export function theirsText(path: string) {
  return pendingDisk.get(path) ?? null;
}
export function keepMine(path: string) {
  const t = pendingDisk.get(path);
  if (t != null) docs.get(path)!.diskText = t;
  pendingDisk.delete(path);
}

export function closeDoc(path: string) {
  const d = docs.get(path);
  if (!d) return;
  docs.delete(path);
  d.model.dispose();
  useEditor.getState().setDirty(path, false);
}

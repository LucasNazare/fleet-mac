// The editor that currently has the user's attention (for global ⌘ shortcuts).
import type { monaco } from "./monaco";

export interface ActiveEditor {
  editor: monaco.editor.IStandaloneCodeEditor;
  path: string;
  root: string;
  projectId: string;
  save: () => Promise<void>;
  inlineEdit: () => void;
}

let current: ActiveEditor | null = null;
export const setActiveEditor = (a: ActiveEditor | null) => {
  current = a;
};
export const activeEditor = () => current;

/** Path relative to the project root, for prompts and headers. */
export function relPath(root: string, path: string) {
  return path.startsWith(root + "/") ? path.slice(root.length + 1) : path;
}

// Monaco, bundled locally (no CDN), with workers and the fleet theme.
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";
import JsonWorker from "monaco-editor/language/json/json.worker.js?worker";
import CssWorker from "monaco-editor/language/css/css.worker.js?worker";
import HtmlWorker from "monaco-editor/language/html/html.worker.js?worker";
import TsWorker from "monaco-editor/language/typescript/ts.worker.js?worker";
import { THEMES } from "../themes/defs";
import { monacoData, monacoThemeId } from "../themes";
import type { ThemeDef } from "../themes/types";

(self as unknown as { MonacoEnvironment: monaco.Environment }).MonacoEnvironment = {
  getWorker(_id: string, label: string) {
    if (label === "json") return new JsonWorker();
    if (label === "css" || label === "scss" || label === "less") return new CssWorker();
    if (label === "html" || label === "handlebars" || label === "razor") return new HtmlWorker();
    if (label === "typescript" || label === "javascript") return new TsWorker();
    return new EditorWorker();
  },
};

for (const t of THEMES) monaco.editor.defineTheme(monacoThemeId(t), monacoData(t));
// Back-compat id for anything still asking for the old theme name.
monaco.editor.defineTheme("fleet-dark", monacoData(THEMES[0]));

let editorFont = THEMES[0].mono;
/** Current monospace stack for editors (follows the theme). */
export const editorFontFamily = () => editorFont;
/** Theme id for new editors. */
export const editorTheme = () => currentEditorTheme;
let currentEditorTheme = monacoThemeId(THEMES[0]);

/** Switch every editor (theme is global in Monaco; fonts per editor). */
export function setEditorTheme(t: ThemeDef) {
  currentEditorTheme = monacoThemeId(t);
  editorFont = t.mono;
  monaco.editor.setTheme(currentEditorTheme);
  for (const ed of monaco.editor.getEditors()) ed.updateOptions({ fontFamily: t.mono });
  for (const d of monaco.editor.getDiffEditors()) d.updateOptions({ fontFamily: t.mono } as never);
}

export const FONT = '"JetBrains Mono", Menlo, monospace';

export function languageFor(path: string): string {
  const name = path.split("/").pop()!.toLowerCase();
  if (name === "dockerfile") return "dockerfile";
  if (name === "makefile") return "makefile";
  const ext = name.includes(".") ? name.split(".").pop()! : "";
  const map: Record<string, string> = {
    ts: "typescript", tsx: "typescript", mts: "typescript", cts: "typescript",
    js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
    json: "json", jsonc: "json", css: "css", scss: "scss", less: "less", html: "html", htm: "html",
    md: "markdown", mdx: "markdown", py: "python", go: "go", rs: "rust", yml: "yaml", yaml: "yaml",
    sql: "sql", sh: "shell", bash: "shell", zsh: "shell", toml: "ini", ini: "ini", rb: "ruby",
    java: "java", kt: "kotlin", swift: "swift", c: "c", h: "c", cpp: "cpp", hpp: "cpp", php: "php",
    xml: "xml", svg: "xml", graphql: "graphql", gql: "graphql", lua: "lua", dart: "dart", vue: "html",
  };
  return map[ext] ?? "plaintext";
}

export { monaco };

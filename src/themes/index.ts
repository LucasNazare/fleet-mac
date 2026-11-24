// Theme registry and the one function that applies a theme everywhere: CSS tokens,
// fonts, terminals, the editor and the native window.
import type { editor } from "monaco-editor";
import { THEMES } from "./defs";
import type { Effects, ThemeDef, ThemeId } from "./types";
import "./effects.css";

export type { Effects, ThemeDef, ThemeId };
export { THEMES };

export function getTheme(id: string | undefined | null): ThemeDef {
  return THEMES.find((t) => t.id === id) ?? THEMES[0];
}

let current: ThemeDef = THEMES[0];
export const currentTheme = () => current;
export const monacoThemeId = (t: ThemeDef = current) => `fleet-${t.id}`;

/** Monaco theme data for a theme. */
export function monacoData(t: ThemeDef): editor.IStandaloneThemeData {
  const s = t.syntax;
  const term = t.terminal;
  const hex = (c: string | undefined, fallback: string) => (c && c.startsWith("#") ? c : fallback);
  return {
    base: t.mode === "dark" ? "vs-dark" : "vs",
    inherit: true,
    rules: [
      { token: "comment", foreground: s.comment, fontStyle: "italic" },
      { token: "string", foreground: s.string },
      { token: "keyword", foreground: s.keyword },
      { token: "number", foreground: s.number },
      { token: "type", foreground: s.type },
    ],
    colors: {
      "editor.background": s.background ? `#${s.background}` : hex(term.background, "#0D1117"),
      "editor.foreground": hex(term.foreground, "#E2E8F0"),
      "editorLineNumber.foreground": `#${s.lineNumber}`,
      "editorLineNumber.activeForeground": hex(term.foreground, "#E2E8F0"),
      "editor.lineHighlightBackground": `#${s.lineHighlight}`,
      "editor.lineHighlightBorder": "#00000000",
      "editor.selectionBackground": `#${s.selection}`,
      "editorCursor.foreground": hex(term.cursor, "#E2E8F0"),
      "editorGutter.background": s.background ? `#${s.background}` : hex(term.background, "#0D1117"),
      "editorWidget.background": `#${s.widget}`,
      "editorWidget.border": `#${s.border}`,
      "editorSuggestWidget.background": `#${s.widget}`,
      "editorHoverWidget.background": `#${s.widget}`,
      "diffEditor.insertedTextBackground": t.mode === "dark" ? "#2EA04326" : "#1A7F3722",
      "diffEditor.removedTextBackground": t.mode === "dark" ? "#F8514926" : "#CF222E1E",
      "diffEditor.insertedLineBackground": t.mode === "dark" ? "#2EA04318" : "#1A7F3712",
      "diffEditor.removedLineBackground": t.mode === "dark" ? "#F8514918" : "#CF222E10",
      "scrollbarSlider.background": t.mode === "dark" ? "#FFFFFF1A" : "#0000001A",
    },
  };
}

/** Black or white text for a chip of this background color (WCAG relative luminance). */
export function readableOn(color: string): string {
  const m = /^#?([0-9a-f]{6})/i.exec(color.trim());
  if (!m) return "var(--chip-text)";
  const n = parseInt(m[1], 16);
  const lin = (v: number) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const L = 0.2126 * lin((n >> 16) & 255) + 0.7152 * lin((n >> 8) & 255) + 0.0722 * lin(n & 255);
  return (L + 0.05) / 0.05 >= 1.05 / (L + 0.05) ? "#0B0D12" : "#FFFFFF";
}

let applied = "";

/** Apply a theme (idempotent). Window appearance goes through `setWindow` (Tauri command). */
export async function applyTheme(id: string, effects: Effects, setWindow: (w: { vibrancy: boolean; dark: boolean; background: string }) => void) {
  const t = getTheme(id);
  const root = document.documentElement;
  root.dataset.effects = effects;
  const key = `${t.id}`;
  if (applied === key) return;
  applied = key;
  current = t;
  // Fonts first, so the terminal's glyph atlas is built from the right face.
  if (t.fonts) {
    await t.fonts().catch(() => {});
    const fam = t.mono.split(",")[0];
    await Promise.race([Promise.all([document.fonts.load(`13px ${fam}`), document.fonts.load(`bold 13px ${fam}`), document.fonts.load(`13px ${t.vars["font-ui"]?.split(",")[0] ?? fam}`)]), new Promise((r) => setTimeout(r, 1200))]).catch(() => {});
  }
  for (const [k, v] of Object.entries(t.vars)) root.style.setProperty(`--${k}`, v);
  root.style.setProperty("color-scheme", t.mode);
  root.dataset.theme = t.id;
  root.dataset.mode = t.mode;
  setWindow({ vibrancy: t.window.vibrancy, dark: t.mode === "dark", background: t.window.background });
  const pool = await import("../terminal/termPool");
  pool.applyTheme(t.terminal, t.mono, !!t.transparent);
  const m = await import("../editor/monaco");
  m.setEditorTheme(t);
}

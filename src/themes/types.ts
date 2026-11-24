import type { ITheme } from "@xterm/xterm";

export type ThemeId = "dark" | "light" | "hacker" | "cyberpunk" | "glass" | "pear" | "retro" | "soft";
export type Effects = "full" | "subtle" | "off";

/** Editor colors as 6/8-digit hex without '#'. */
export interface Syntax {
  comment: string;
  string: string;
  keyword: string;
  number: string;
  type: string;
  lineNumber: string;
  lineHighlight: string;
  selection: string;
  widget: string;
  border: string;
  /** Editor background when it differs from the terminal's. */
  background?: string;
}

export interface ThemeDef {
  id: ThemeId;
  name: string;
  blurb: string;
  mode: "dark" | "light";
  /** CSS custom properties (without the leading --). */
  vars: Record<string, string>;
  /** Monospace stack for terminals and the editor. */
  mono: string;
  /** Loads the theme's bundled fonts (lazy). */
  fonts?: () => Promise<unknown>;
  terminal: ITheme;
  syntax: Syntax;
  window: { vibrancy: boolean; background: string };
  /** Translucent terminals/editor (Glass). */
  transparent?: boolean;
}

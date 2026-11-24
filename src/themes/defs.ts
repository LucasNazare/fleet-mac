// The themes. Each one sets every token in styles.css (:root), the terminal and editor
// colors, fonts and the native window treatment. Classic Dark mirrors styles.css exactly.
import type { ThemeDef, Syntax } from "./types";

const CLASSIC_DARK_VARS: Record<string, string> = {
  bg: "#0D1117", "app-bg": "#0D1117", "term-bg": "#0D1117", panel: "#12171E", "panel-2": "#171D26", hover: "#1A212B", sel: "#1E2733",
  border: "#252D38", "border-strong": "#3A4554", text: "#E2E8F0", muted: "#94A0B0", accent: "#58A6FF",
  amber: "#E3B341", blue: "#58A6FF", green: "#3FB950", red: "#FF7B72", violet: "#D2A8FF", cyan: "#56D4DD",
  "state-needs": "#E3B341", "state-working": "#58A6FF", "state-done": "#3FB950", "state-failed": "#FF7B72", "state-idle": "#94A0B0",
  "primary-bg": "#1F3A5F", "primary-bg-hover": "#24456F", "primary-border": "#2E5A8F", "primary-text": "inherit",
  "warn-bg": "#2A2112", "warn-bg-2": "#1E1A10", "warn-border": "#4A3B17",
  "danger-bg": "#2A1617", "danger-bg-2": "#241517", "danger-text": "#FFB4AE", "danger-border": "#6B2B2B", "danger-soft": "rgba(255, 123, 114, .12)",
  "inline-added": "rgba(46, 160, 67, .18)", "inline-removed": "rgba(248, 81, 73, .14)",
  overlay: "rgba(1, 4, 9, .55)", "shadow-md": "0 10px 36px rgba(0, 0, 0, .45)", "shadow-lg": "0 20px 60px rgba(0, 0, 0, .5)",
  "chip-text": "#0D1117", "radius-sm": "5px", radius: "6px", "radius-lg": "10px", "row-pad-y": "6px",
  "lane-1": "#58A6FF", "lane-2": "#FFA657", "lane-3": "#7EE787", "lane-4": "#D2A8FF", "lane-5": "#FF7B72", "lane-6": "#56D4DD", "lane-7": "#E3B341", "lane-8": "#DB61A2",
  font: '"JetBrains Mono", Menlo, monospace', "font-ui": '"JetBrains Mono", Menlo, monospace', "font-display": '"JetBrains Mono", Menlo, monospace',
};

const lanes = (c: string[]) => Object.fromEntries(c.map((x, i) => [`lane-${i + 1}`, x]));
const JB = '"JetBrains Mono", Menlo, monospace';

const classicDark: ThemeDef = {
  id: "dark",
  name: "Classic Dark",
  blurb: "The original: calm, dense, all monospace.",
  mode: "dark",
  vars: CLASSIC_DARK_VARS,
  mono: JB,
  terminal: {
    background: "#0D1117", foreground: "#E2E8F0", cursor: "#E2E8F0", cursorAccent: "#0D1117", selectionBackground: "#2B3A4F",
    black: "#1B222C", red: "#FF7B72", green: "#7EE787", yellow: "#E3B341", blue: "#58A6FF", magenta: "#D2A8FF", cyan: "#56D4DD", white: "#C9D1D9",
    brightBlack: "#6E7681", brightRed: "#FFA198", brightGreen: "#A5F0B0", brightYellow: "#F2CC60", brightBlue: "#79C0FF", brightMagenta: "#E2C5FF", brightCyan: "#7CE4EC", brightWhite: "#F0F6FC",
  },
  syntax: { comment: "8B96A5", string: "A5D6FF", keyword: "FF7B72", number: "79C0FF", type: "FFA657", lineNumber: "5A6472", lineHighlight: "141A22", selection: "2B3A4F", widget: "12171E", border: "252D38" },
  window: { vibrancy: false, background: "#0D1117" },
};

const classicLight: ThemeDef = {
  id: "light",
  name: "Classic Light",
  blurb: "The original, on paper white.",
  mode: "light",
  vars: {
    ...CLASSIC_DARK_VARS,
    bg: "#FFFFFF", "app-bg": "#FFFFFF", "term-bg": "#FFFFFF", panel: "#F6F8FA", "panel-2": "#EEF1F4", hover: "#EAEEF2", sel: "#DDE6F0",
    border: "#D0D7DE", "border-strong": "#AFB8C1", text: "#1F2328", muted: "#59636E", accent: "#0969DA",
    amber: "#9A6700", blue: "#0969DA", green: "#1A7F37", red: "#CF222E", violet: "#8250DF", cyan: "#1B7C83",
    "state-needs": "#9A6700", "state-working": "#0969DA", "state-done": "#1A7F37", "state-failed": "#CF222E", "state-idle": "#59636E",
    "primary-bg": "#DDF4FF", "primary-bg-hover": "#B6E3FF", "primary-border": "#54AEFF",
    "warn-bg": "#FFF8C5", "warn-bg-2": "#FFFBE6", "warn-border": "#D4A72C",
    "danger-bg": "#FFEBE9", "danger-bg-2": "#FFF5F4", "danger-text": "#A40E26", "danger-border": "#FF8182", "danger-soft": "rgba(207, 34, 46, .08)",
    "inline-added": "rgba(26, 127, 55, .14)", "inline-removed": "rgba(207, 34, 46, .1)",
    overlay: "rgba(27, 31, 36, .35)", "shadow-md": "0 8px 24px rgba(140, 149, 159, .25)", "shadow-lg": "0 16px 48px rgba(140, 149, 159, .35)",
    ...lanes(["#0969DA", "#BC4C00", "#1A7F37", "#8250DF", "#CF222E", "#1B7C83", "#9A6700", "#BF3989"]),
  },
  mono: JB,
  terminal: {
    background: "#FFFFFF", foreground: "#1F2328", cursor: "#1F2328", cursorAccent: "#FFFFFF", selectionBackground: "#B6E3FF",
    black: "#24292F", red: "#CF222E", green: "#116329", yellow: "#4D2D00", blue: "#0969DA", magenta: "#8250DF", cyan: "#1B7C83", white: "#6E7781",
    brightBlack: "#57606A", brightRed: "#A40E26", brightGreen: "#1A7F37", brightYellow: "#633C01", brightBlue: "#218BFF", brightMagenta: "#A475F9", brightCyan: "#3192AA", brightWhite: "#424A53",
  },
  syntax: { comment: "6E7781", string: "0A3069", keyword: "CF222E", number: "0550AE", type: "953800", lineNumber: "8C959F", lineHighlight: "F6F8FA", selection: "B6E3FF", widget: "F6F8FA", border: "D0D7DE" },
  window: { vibrancy: false, background: "#FFFFFF" },
};

const hacker: ThemeDef = {
  id: "hacker",
  name: "Hacker",
  blurb: "Phosphor green on black. Square, dense, scanlines.",
  mode: "dark",
  vars: {
    ...CLASSIC_DARK_VARS,
    bg: "#050805", "app-bg": "#050805", "term-bg": "#050805", panel: "#070B07", "panel-2": "#0B110B", hover: "#0E1A0F", sel: "#13281A",
    border: "#1B3A20", "border-strong": "#2E6B36", text: "#7DF59A", muted: "#46A85C", accent: "#33FF66",
    amber: "#FFB000", blue: "#4DE8FF", green: "#33FF66", red: "#FF5252", violet: "#B98CFF", cyan: "#4DE8FF",
    "state-needs": "#FFB000", "state-working": "#4DE8FF", "state-done": "#33FF66", "state-failed": "#FF5252", "state-idle": "#46A85C",
    "primary-bg": "#0F2E15", "primary-bg-hover": "#154020", "primary-border": "#33FF66", "primary-text": "#33FF66",
    "warn-bg": "#1F1800", "warn-bg-2": "#141000", "warn-border": "#6B4F00",
    "danger-bg": "#260808", "danger-bg-2": "#1C0707", "danger-text": "#FF8C8C", "danger-border": "#6B1A1A", "danger-soft": "rgba(255, 82, 82, .12)",
    "inline-added": "rgba(51, 255, 102, .12)", "inline-removed": "rgba(255, 82, 82, .12)",
    overlay: "rgba(0, 0, 0, .72)", "shadow-md": "0 0 0 1px #1B3A20", "shadow-lg": "0 0 0 1px #2E6B36, 0 0 24px rgba(51, 255, 102, .12)",
    "chip-text": "#050805", "radius-sm": "0px", radius: "0px", "radius-lg": "0px", "row-pad-y": "3px",
    ...lanes(["#33FF66", "#4DE8FF", "#FFB000", "#B98CFF", "#FF5252", "#8CFFA6", "#FFD166", "#80C4FF"]),
    font: '"IBM Plex Mono", Menlo, monospace', "font-ui": '"IBM Plex Mono", Menlo, monospace', "font-display": '"IBM Plex Mono", Menlo, monospace',
  },
  mono: '"IBM Plex Mono", Menlo, monospace',
  fonts: () => Promise.all([import("@fontsource/ibm-plex-mono/400.css"), import("@fontsource/ibm-plex-mono/500.css"), import("@fontsource/ibm-plex-mono/700.css")]),
  terminal: {
    background: "#050805", foreground: "#7DF59A", cursor: "#33FF66", cursorAccent: "#050805", selectionBackground: "#1F4D2A",
    black: "#0A140B", red: "#FF5252", green: "#33FF66", yellow: "#FFB000", blue: "#4DA6FF", magenta: "#C77DFF", cyan: "#4DE8FF", white: "#B8FFC6",
    brightBlack: "#46A85C", brightRed: "#FF8080", brightGreen: "#8CFFA6", brightYellow: "#FFD166", brightBlue: "#80C4FF", brightMagenta: "#DDB0FF", brightCyan: "#8CF2FF", brightWhite: "#E6FFEB",
  },
  syntax: { comment: "46A85C", string: "B8FFC6", keyword: "33FF66", number: "FFB000", type: "4DE8FF", lineNumber: "2E6B36", lineHighlight: "0B140C", selection: "1F4D2A", widget: "070B07", border: "1B3A20" },
  window: { vibrancy: false, background: "#050805" },
};

const cyberpunk: ThemeDef = {
  id: "cyberpunk",
  name: "Cyberpunk",
  blurb: "Night City neon: signal yellow, hot cyan, chamfered edges.",
  mode: "dark",
  vars: {
    ...CLASSIC_DARK_VARS,
    bg: "#0A0B14", "app-bg": "#0A0B14", "term-bg": "#07080F", panel: "#0E1020", "panel-2": "#13162A", hover: "#1A1E38", sel: "#232853",
    border: "#2A2F55", "border-strong": "#4A4F8A", text: "#E8F1FF", muted: "#8F98CC", accent: "#FCEE0A",
    amber: "#FCEE0A", blue: "#00F0FF", green: "#2BFFA0", red: "#FF2E63", violet: "#C561FF", cyan: "#00F0FF",
    "state-needs": "#FCEE0A", "state-working": "#00F0FF", "state-done": "#2BFFA0", "state-failed": "#FF2E63", "state-idle": "#8F98CC",
    "primary-bg": "#FCEE0A", "primary-bg-hover": "#FFF35C", "primary-border": "#FCEE0A", "primary-text": "#0A0B14",
    "warn-bg": "#2A2700", "warn-bg-2": "#1A1800", "warn-border": "#8A8000",
    "danger-bg": "#2A0812", "danger-bg-2": "#1F0710", "danger-text": "#FF7A9A", "danger-border": "#8A0F2E", "danger-soft": "rgba(255, 46, 99, .14)",
    "inline-added": "rgba(43, 255, 160, .12)", "inline-removed": "rgba(255, 46, 99, .14)",
    overlay: "rgba(5, 6, 14, .78)", "shadow-md": "0 0 0 1px #2A2F55, 0 12px 32px rgba(0, 0, 0, .6)", "shadow-lg": "0 0 0 1px rgba(252, 238, 10, .35), 0 0 32px rgba(0, 240, 255, .12), 0 24px 64px rgba(0, 0, 0, .7)",
    "chip-text": "#0A0B14", "radius-sm": "0px", radius: "0px", "radius-lg": "0px", "row-pad-y": "6px",
    ...lanes(["#00F0FF", "#FCEE0A", "#FF2BD6", "#2BFFA0", "#FF2E63", "#C561FF", "#4DC1FF", "#FFF56B"]),
    font: JB, "font-ui": '"Rajdhani", "Avenir Next Condensed", sans-serif', "font-display": '"Orbitron", "Rajdhani", sans-serif',
  },
  mono: JB,
  fonts: () => Promise.all([import("@fontsource/rajdhani/500.css"), import("@fontsource/rajdhani/600.css"), import("@fontsource/rajdhani/700.css"), import("@fontsource/orbitron/600.css"), import("@fontsource/orbitron/700.css")]),
  terminal: {
    background: "#07080F", foreground: "#E8F1FF", cursor: "#FCEE0A", cursorAccent: "#07080F", selectionBackground: "#3A2F6B",
    black: "#12142A", red: "#FF2E63", green: "#2BFFA0", yellow: "#FCEE0A", blue: "#00A3FF", magenta: "#FF2BD6", cyan: "#00F0FF", white: "#D8E2FF",
    brightBlack: "#5A609A", brightRed: "#FF5C85", brightGreen: "#7DFFC4", brightYellow: "#FFF56B", brightBlue: "#4DC1FF", brightMagenta: "#FF6BE4", brightCyan: "#6BF7FF", brightWhite: "#FFFFFF",
  },
  syntax: { comment: "6B73A8", string: "00F0FF", keyword: "FCEE0A", number: "FF2BD6", type: "2BFFA0", lineNumber: "4A4F8A", lineHighlight: "11132A", selection: "3A2F6B", widget: "0E1020", border: "2A2F55" },
  window: { vibrancy: false, background: "#0A0B14" },
};

const glass: ThemeDef = {
  id: "glass",
  name: "Glass",
  blurb: "See-through frosted window; your wallpaper glows through.",
  mode: "dark",
  vars: {
    ...CLASSIC_DARK_VARS,
    bg: "rgba(16, 18, 28, .42)", "app-bg": "transparent", "term-bg": "rgba(10, 12, 18, .62)", panel: "rgba(255, 255, 255, .055)", "panel-2": "rgba(30, 34, 48, .72)",
    hover: "rgba(255, 255, 255, .08)", sel: "rgba(255, 255, 255, .15)", border: "rgba(255, 255, 255, .12)", "border-strong": "rgba(255, 255, 255, .26)",
    text: "#F4F6FB", muted: "#B4BCD0", accent: "#8AB4FF",
    amber: "#FFCB6B", blue: "#8AB4FF", green: "#6EE7A8", red: "#FF8A8A", violet: "#C9A7FF", cyan: "#7DE3F0",
    "state-needs": "#FFCB6B", "state-working": "#8AB4FF", "state-done": "#6EE7A8", "state-failed": "#FF8A8A", "state-idle": "#B4BCD0",
    "primary-bg": "rgba(138, 180, 255, .26)", "primary-bg-hover": "rgba(138, 180, 255, .36)", "primary-border": "rgba(138, 180, 255, .55)",
    "warn-bg": "rgba(255, 203, 107, .14)", "warn-bg-2": "rgba(255, 203, 107, .09)", "warn-border": "rgba(255, 203, 107, .38)",
    "danger-bg": "rgba(255, 138, 138, .14)", "danger-bg-2": "rgba(255, 138, 138, .09)", "danger-text": "#FFC2C2", "danger-border": "rgba(255, 138, 138, .4)", "danger-soft": "rgba(255, 138, 138, .12)",
    overlay: "rgba(0, 0, 0, .22)", "shadow-md": "0 12px 40px rgba(0, 0, 0, .35)", "shadow-lg": "0 30px 90px rgba(0, 0, 0, .45)",
    "radius-sm": "8px", radius: "10px", "radius-lg": "16px", "row-pad-y": "6px",
    ...lanes(["#8AB4FF", "#FFCB6B", "#6EE7A8", "#C9A7FF", "#FF8A8A", "#7DE3F0", "#FFB38A", "#F0A6E0"]),
    "font-ui": '"Inter Variable", "Inter", -apple-system, sans-serif', "font-display": '"Inter Variable", "Inter", -apple-system, sans-serif',
  },
  mono: JB,
  fonts: () => import("@fontsource-variable/inter/index.css"),
  terminal: {
    background: "#00000000", foreground: "#F4F6FB", cursor: "#8AB4FF", cursorAccent: "#0A0C12", selectionBackground: "#8AB4FF55",
    black: "#1C2030", red: "#FF8A8A", green: "#6EE7A8", yellow: "#FFCB6B", blue: "#8AB4FF", magenta: "#C9A7FF", cyan: "#7DE3F0", white: "#DCE2F0",
    brightBlack: "#7A839A", brightRed: "#FFB0B0", brightGreen: "#9CF0C4", brightYellow: "#FFDC94", brightBlue: "#B0CCFF", brightMagenta: "#DCC4FF", brightCyan: "#A6EDF6", brightWhite: "#FFFFFF",
  },
  syntax: { comment: "8C95AC", string: "9CF0C4", keyword: "C9A7FF", number: "FFCB6B", type: "8AB4FF", lineNumber: "6A7390", lineHighlight: "FFFFFF0D", selection: "8AB4FF40", widget: "1E2230", border: "FFFFFF20", background: "0A0C129E" },
  window: { vibrancy: true, background: "#00000000" },
  transparent: true,
};

const pear: ThemeDef = {
  id: "pear",
  name: "Pear",
  blurb: "Bright, native and quiet — system type, hairlines, one blue.",
  mode: "light",
  vars: {
    ...CLASSIC_DARK_VARS,
    bg: "#FFFFFF", "app-bg": "#FFFFFF", "term-bg": "#FFFFFF", panel: "#F5F5F7", "panel-2": "#FFFFFF", hover: "#EBEBEE", sel: "#E1EBFB",
    border: "#E3E3E6", "border-strong": "#C7C7CC", text: "#1D1D1F", muted: "#6E6E73", accent: "#0071E3",
    amber: "#B35A00", blue: "#0071E3", green: "#1E7F3A", red: "#D70015", violet: "#8944AB", cyan: "#0E7C86",
    "state-needs": "#B35A00", "state-working": "#0071E3", "state-done": "#1E7F3A", "state-failed": "#D70015", "state-idle": "#6E6E73",
    "primary-bg": "#0071E3", "primary-bg-hover": "#0077ED", "primary-border": "#0071E3", "primary-text": "#FFFFFF",
    "warn-bg": "#FFF4E5", "warn-bg-2": "#FFF8EE", "warn-border": "#F5C98A",
    "danger-bg": "#FFECEC", "danger-bg-2": "#FFF4F4", "danger-text": "#B00012", "danger-border": "#F3A3A3", "danger-soft": "rgba(215, 0, 21, .08)",
    "inline-added": "rgba(30, 127, 58, .13)", "inline-removed": "rgba(215, 0, 21, .1)",
    overlay: "rgba(0, 0, 0, .18)", "shadow-md": "0 8px 28px rgba(0, 0, 0, .12)", "shadow-lg": "0 24px 64px rgba(0, 0, 0, .18), 0 0 0 .5px rgba(0, 0, 0, .12)",
    "radius-sm": "6px", radius: "8px", "radius-lg": "12px", "row-pad-y": "7px",
    ...lanes(["#0071E3", "#C75100", "#1E7F3A", "#8944AB", "#D70015", "#0E7C86", "#A05A00", "#C2185B"]),
    font: '"SF Mono", ui-monospace, Menlo, monospace', "font-ui": '-apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", sans-serif', "font-display": '-apple-system, BlinkMacSystemFont, "SF Pro Display", "Helvetica Neue", sans-serif',
  },
  mono: '"SF Mono", Menlo, monospace',
  terminal: {
    background: "#FFFFFF", foreground: "#1D1D1F", cursor: "#0071E3", cursorAccent: "#FFFFFF", selectionBackground: "#B3D7FF",
    black: "#1D1D1F", red: "#C41A16", green: "#1E7F3A", yellow: "#7A5B00", blue: "#0E4BC1", magenta: "#A90D91", cyan: "#0E7C86", white: "#6E6E73",
    brightBlack: "#8E8E93", brightRed: "#E0251F", brightGreen: "#248A3D", brightYellow: "#8F6A00", brightBlue: "#0071E3", brightMagenta: "#BF2FB0", brightCyan: "#11869A", brightWhite: "#3A3A3C",
  },
  syntax: { comment: "8E8E93", string: "C41A16", keyword: "AD3DA4", number: "1C00CF", type: "0B4F79", lineNumber: "AEAEB2", lineHighlight: "F5F5F7", selection: "B3D7FF", widget: "FFFFFF", border: "E3E3E6" },
  window: { vibrancy: false, background: "#FFFFFF" },
};

const retro: ThemeDef = {
  id: "retro",
  name: "Retro-Futuristic",
  blurb: "Space-age optimism: indigo night, sunset chrome, round shapes.",
  mode: "dark",
  vars: {
    ...CLASSIC_DARK_VARS,
    bg: "#140F2D", "app-bg": "#120D29", "term-bg": "#100B24", panel: "#1B1438", "panel-2": "#231A47", hover: "#2A2054", sel: "#352866",
    border: "#3A2E6E", "border-strong": "#5A4A9C", text: "#F6EBD9", muted: "#B9ADDF", accent: "#FF8A3D",
    amber: "#FFB347", blue: "#6FB7FF", green: "#7CF2B0", red: "#FF6B85", violet: "#C49BFF", cyan: "#5BE7E0",
    "state-needs": "#FFB347", "state-working": "#5BE7E0", "state-done": "#7CF2B0", "state-failed": "#FF6B85", "state-idle": "#B9ADDF",
    "primary-bg": "linear-gradient(95deg, #FF8A3D, #FF4F8B 55%, #9B5CFF)", "primary-bg-hover": "linear-gradient(95deg, #FFA05C, #FF6A9D 55%, #AE78FF)", "primary-border": "transparent", "primary-text": "#1A1030",
    "warn-bg": "#33240F", "warn-bg-2": "#251A0D", "warn-border": "#8A5A1A",
    "danger-bg": "#3A1426", "danger-bg-2": "#2C1020", "danger-text": "#FFB0C0", "danger-border": "#8A2E4E", "danger-soft": "rgba(255, 107, 133, .14)",
    "inline-added": "rgba(124, 242, 176, .14)", "inline-removed": "rgba(255, 107, 133, .14)",
    overlay: "rgba(10, 6, 25, .62)", "shadow-md": "0 12px 32px rgba(5, 2, 20, .55)", "shadow-lg": "0 24px 70px rgba(5, 2, 20, .65), 0 0 0 1px rgba(255, 138, 61, .18)",
    "chip-text": "#1A1030", "radius-sm": "10px", radius: "14px", "radius-lg": "22px", "row-pad-y": "6px",
    ...lanes(["#FF8A3D", "#FF4F8B", "#9B5CFF", "#5BE7E0", "#FFB347", "#7CF2B0", "#6FB7FF", "#FF6B85"]),
    "font-ui": '"Space Grotesk", "Avenir Next", sans-serif', "font-display": '"Audiowide", "Space Grotesk", sans-serif',
  },
  mono: JB,
  fonts: () => Promise.all([import("@fontsource/space-grotesk/400.css"), import("@fontsource/space-grotesk/500.css"), import("@fontsource/space-grotesk/700.css"), import("@fontsource/audiowide/400.css")]),
  terminal: {
    background: "#100B24", foreground: "#F6EBD9", cursor: "#FF8A3D", cursorAccent: "#100B24", selectionBackground: "#3D2C77",
    black: "#1B1438", red: "#FF6B85", green: "#7CF2B0", yellow: "#FFB347", blue: "#6FB7FF", magenta: "#FF4F8B", cyan: "#5BE7E0", white: "#E6DCC8",
    brightBlack: "#6E61A3", brightRed: "#FF94A7", brightGreen: "#A8F7CC", brightYellow: "#FFCA7A", brightBlue: "#9CCEFF", brightMagenta: "#FF85AE", brightCyan: "#8CF0EB", brightWhite: "#FFF8EC",
  },
  syntax: { comment: "7D70B3", string: "FFB347", keyword: "FF4F8B", number: "5BE7E0", type: "C49BFF", lineNumber: "5A4A9C", lineHighlight: "1A1436", selection: "3D2C77", widget: "1B1438", border: "3A2E6E" },
  window: { vibrancy: false, background: "#120D29" },
};

const soft: ThemeDef = {
  id: "soft",
  name: "Soft",
  blurb: "Neomorphism: one soft grey, extruded controls, pressed selections.",
  mode: "light",
  vars: {
    ...CLASSIC_DARK_VARS,
    bg: "#E4E9F0", "app-bg": "#E4E9F0", "term-bg": "#E9EDF3", panel: "#E4E9F0", "panel-2": "#E4E9F0", hover: "#DCE2EA", sel: "#E4E9F0",
    border: "rgba(150, 165, 188, .28)", "border-strong": "#BBC5D3", text: "#2B3647", muted: "#566175", accent: "#5361E8",
    amber: "#9A5600", blue: "#3D55E0", green: "#1C774A", red: "#BE2E46", violet: "#7447D1", cyan: "#0E6F7D",
    "state-needs": "#9A5600", "state-working": "#3D55E0", "state-done": "#1C774A", "state-failed": "#BE2E46", "state-idle": "#566175",
    "primary-bg": "#5361E8", "primary-bg-hover": "#4654DE", "primary-border": "transparent", "primary-text": "#FFFFFF",
    "warn-bg": "#F2E6D2", "warn-bg-2": "#EFE7DA", "warn-border": "transparent",
    "danger-bg": "#F1DADF", "danger-bg-2": "#EEDFE3", "danger-text": "#9A1F34", "danger-border": "transparent", "danger-soft": "rgba(190, 46, 70, .08)",
    "inline-added": "rgba(28, 119, 74, .12)", "inline-removed": "rgba(190, 46, 70, .1)",
    overlay: "rgba(196, 205, 218, .55)", "shadow-md": "8px 8px 18px #C2CAD6, -8px -8px 18px #FFFFFF", "shadow-lg": "16px 16px 36px #BCC5D2, -16px -16px 36px #FFFFFF",
    "radius-sm": "10px", radius: "14px", "radius-lg": "22px", "row-pad-y": "8px",
    ...lanes(["#3D55E0", "#B35A00", "#1C774A", "#7447D1", "#BE2E46", "#0E6F7D", "#8A6A00", "#B0307A"]),
    "font-ui": '"Manrope Variable", "Manrope", -apple-system, sans-serif', "font-display": '"Manrope Variable", "Manrope", -apple-system, sans-serif',
  },
  mono: JB,
  fonts: () => import("@fontsource-variable/manrope/index.css"),
  terminal: {
    background: "#E9EDF3", foreground: "#2B3647", cursor: "#5361E8", cursorAccent: "#E9EDF3", selectionBackground: "#C6CEF6",
    black: "#2B3647", red: "#BE2E46", green: "#1C774A", yellow: "#7A5600", blue: "#3D55E0", magenta: "#9A3FB8", cyan: "#0E6F7D", white: "#566175",
    brightBlack: "#7A8597", brightRed: "#D23C55", brightGreen: "#21885A", brightYellow: "#8F6800", brightBlue: "#5361E8", brightMagenta: "#AD4FCC", brightCyan: "#11808F", brightWhite: "#3A4558",
  },
  syntax: { comment: "7A8597", string: "1C774A", keyword: "7447D1", number: "B35A00", type: "3D55E0", lineNumber: "A3AEBE", lineHighlight: "E1E6EE", selection: "C6CEF6", widget: "E4E9F0", border: "C9D1DD" },
  window: { vibrancy: false, background: "#E4E9F0" },
};

export const THEMES: ThemeDef[] = [classicDark, classicLight, hacker, cyberpunk, glass, pear, retro, soft];
export type { Syntax };

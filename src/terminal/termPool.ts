// One xterm.js instance per session, created once and kept alive for the life of
// the session. Switching sessions only moves the terminal's DOM element between
// the offscreen pool and a visible host, so it is effectively instant.
// Only mounted terminals get the WebGL renderer (browsers cap WebGL contexts).
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import { backend, invoke } from "../ipc";
import type { ITheme } from "@xterm/xterm";
import { THEMES } from "../themes/defs";

interface Entry {
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  el: HTMLDivElement;
  webgl: WebglAddon | null;
  host: HTMLElement | null;
  connected: boolean;
}

const pool = new Map<string, Entry>();
/** Terminals that keep their WebGL renderer, most recently shown last. */
const glLru: string[] = [];
const MAX_GL = 8;

function touchGl(id: string) {
  const i = glLru.indexOf(id);
  if (i >= 0) glLru.splice(i, 1);
  glLru.push(id);
  // Evict the least recently shown terminals that are not on screen.
  for (let j = 0; glLru.length > MAX_GL && j < glLru.length; ) {
    const e = pool.get(glLru[j]);
    if (e && e.host) { j++; continue; }
    e?.webgl?.dispose();
    if (e) e.webgl = null;
    glLru.splice(j, 1);
  }
}
let holder: HTMLDivElement | null = null;
export const FONT_SIZE = 13;

function getHolder(): HTMLDivElement {
  if (!holder) {
    holder = document.createElement("div");
    holder.id = "term-pool";
    // Offscreen but sized: xterm keeps parsing output; its renderer pauses while not visible.
    Object.assign(holder.style, { position: "fixed", left: "-20000px", top: "0", width: "1000px", height: "600px", overflow: "hidden" });
    document.body.appendChild(holder);
  }
  return holder;
}

let lastSize = { cols: 120, rows: 32 };
let termLook: { theme: ITheme; mono: string; transparent: boolean } = { theme: THEMES[0].terminal, mono: THEMES[0].mono, transparent: false };

/** Apply a theme's terminal colors and font to every live terminal (and new ones). */
export function applyTheme(theme: ITheme, mono: string, transparent: boolean) {
  const changedTransparency = termLook.transparent !== transparent;
  termLook = { theme, mono, transparent };
  for (const [id, e] of pool) {
    e.term.options.theme = theme;
    if (e.term.options.fontFamily !== mono) e.term.options.fontFamily = mono;
    if (changedTransparency) {
      // The WebGL renderer is built for one transparency mode: rebuild it.
      e.term.options.allowTransparency = transparent;
      e.webgl?.dispose();
      e.webgl = null;
      if (e.host) mount(id, e.host, false);
    }
    fitNow(id);
    e.term.refresh(0, e.term.rows - 1);
  }
}
let termSettings = { scrollback: 10000, cursorStyle: "block" as "block" | "bar" | "underline", copyOnSelect: false, optionAsMeta: true, fontSize: FONT_SIZE };

/** Apply terminal settings to every live terminal. */
export function applySettings(t: { scrollback: number; cursorStyle: "block" | "bar" | "underline"; copyOnSelect: boolean; optionAsMeta: boolean }, fontSize: number) {
  termSettings = { ...t, fontSize };
  for (const [id, e] of pool) {
    e.term.options.scrollback = t.scrollback;
    e.term.options.cursorStyle = t.cursorStyle;
    e.term.options.macOptionIsMeta = t.optionAsMeta;
    if (e.term.options.fontSize !== fontSize) {
      e.term.options.fontSize = fontSize;
      fitNow(id);
    }
  }
}
/** Best guess of a terminal size for a new PTY (the size of the last fitted terminal). */
export function currentSize() {
  return lastSize;
}

function ensure(id: string): Entry {
  let e = pool.get(id);
  if (e) return e;
  const term = new Terminal({
    fontFamily: termLook.mono,
    fontSize: termSettings.fontSize,
    lineHeight: 1.15,
    scrollback: termSettings.scrollback,
    cursorStyle: termSettings.cursorStyle,
    allowProposedApi: true,
    macOptionIsMeta: termSettings.optionAsMeta,
    macOptionClickForcesSelection: true,
    cursorBlink: false,
    theme: termLook.theme,
    allowTransparency: termLook.transparent,
    cols: lastSize.cols,
    rows: lastSize.rows,
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  term.loadAddon(fit);
  term.loadAddon(search);
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = "11";
  term.loadAddon(new WebLinksAddon((_ev, uri) => void backend().then((b) => b.openUrl(uri))));
  // ⌘-combos belong to the app; everything else (all Ctrl keys) goes to the PTY.
  term.attachCustomKeyEventHandler((ev) => {
    if (ev.metaKey) return false;
    // Keys the input method is handling (dead keys, IME): their text comes from textInput().
    if (ev.isComposing || ev.keyCode === 229 || ev.key === "Dead") return false;
    // Shift+Enter → newline in Claude Code's prompt. xterm.js would send a plain CR
    // (= submit); send ESC+CR instead, which Claude Code treats as a line break
    // (same as Option+Enter).
    if (ev.key === "Enter" && ev.shiftKey && !ev.ctrlKey && !ev.altKey) {
      if (ev.type === "keydown") void invoke("pty_write", { id, data: "\x1b\r" }).catch(() => {});
      ev.preventDefault();
      return false;
    }
    return true;
  });
  term.onData((data) => void invoke("pty_write", { id, data }).catch(() => {}));
  term.onBinary((data) => void invoke("pty_write", { id, data }).catch(() => {}));
  term.onSelectionChange(() => {
    if (termSettings.copyOnSelect && term.hasSelection()) void navigator.clipboard.writeText(term.getSelection()).catch(() => {});
  });
  term.onResize(({ cols, rows }) => void invoke("pty_resize", { id, cols, rows }).catch(() => {}));

  const el = document.createElement("div");
  el.className = "term-el";
  el.dataset.session = id;
  getHolder().appendChild(el);
  textInput(el, (data) => term.input(data, true));
  term.open(el);
  e = { term, fit, search, el, webgl: null, host: null, connected: false };
  pool.set(id, e);
  return e;
}

/**
 * Text from the macOS input method: accents via dead keys (' " ` ~ ^ on ABNT2 / US-International),
 * Option-combos and IMEs. xterm.js's composition handling misreads WebKit's event order (' then r
 * came out as ''), so these events never reach it: the text is sent once, as the input method
 * commits it (compositionend), and plain inserted text (input/insertText) as it arrives.
 * Capture listeners on the terminal's container run before xterm's own on its textarea.
 * `localStorage["fleet.keyTrace"] = "1"` logs the events to the console.
 */
function textInput(el: HTMLElement, send: (data: string) => void) {
  let composing = false;
  const trace = (ev: Event) => {
    if (!localStorage.getItem("fleet.keyTrace")) return;
    if (ev instanceof KeyboardEvent) console.log("[fleet key]", ev.type, ev.key, ev.keyCode, ev.isComposing ? "composing" : "");
    else console.log("[fleet key]", ev.type, (ev as InputEvent).inputType ?? "", JSON.stringify((ev as InputEvent).data ?? null));
  };
  const own = (ev: Event) => {
    if (!(ev.target instanceof HTMLTextAreaElement)) return false;
    trace(ev);
    ev.stopImmediatePropagation();
    return true;
  };
  el.addEventListener("compositionstart", (ev) => { if (own(ev)) composing = true; }, true);
  el.addEventListener("compositionupdate", own, true);
  el.addEventListener("compositionend", (ev) => {
    if (!own(ev)) return;
    composing = false;
    const data = (ev as CompositionEvent).data;
    if (data) send(data);
    const ta = ev.target as HTMLTextAreaElement;
    setTimeout(() => { if (!composing) ta.value = ""; }, 0);
  }, true);
  el.addEventListener("input", (ev) => {
    if (!own(ev)) return;
    const ie = ev as InputEvent;
    if (!composing && ie.inputType === "insertText" && ie.data) send(ie.data);
  }, true);
  el.addEventListener("keydown", (ev) => { if (ev.target instanceof HTMLTextAreaElement) trace(ev); }, true);
}

/** Subscribe to the session's PTY output (replays the backend's buffer first). */
export async function connect(id: string): Promise<boolean> {
  const e = ensure(id);
  if (e.connected) return true;
  e.connected = true;
  const be = await backend();
  const ch = be.channel();
  ch.onmessage = (data) => {
    if (typeof data === "string") e.term.write(data);
    else e.term.write(data instanceof Uint8Array ? data : new Uint8Array(data));
  };
  const ok = await invoke<boolean>("pty_attach", { id, onData: ch.raw }).catch(() => false);
  if (!ok) e.connected = false;
  return ok;
}

export function mount(id: string, host: HTMLElement, focus = true) {
  const e = ensure(id);
  const t0 = performance.now();
  if (e.host !== host) {
    host.appendChild(e.el);
    e.host = host;
  }
  if (!e.webgl) {
    try {
      const gl = new WebglAddon();
      gl.onContextLoss(() => {
        gl.dispose();
        e.webgl = null;
      });
      e.term.loadAddon(gl);
      e.webgl = gl;
    } catch {
      e.webgl = null; // DOM renderer fallback
    }
  }
  touchGl(id);
  fitNow(id);
  e.term.refresh(0, e.term.rows - 1);
  if (focus) e.term.focus();
  requestAnimationFrame(() => {
    if (import.meta.env.DEV) console.debug(`[fleet] switch to ${id.slice(0, 8)} in ${(performance.now() - t0).toFixed(1)} ms`);
  });
}

export function unmount(id: string, host: HTMLElement) {
  const e = pool.get(id);
  if (!e || e.host !== host) return;
  // Keep the WebGL renderer (LRU-capped) so switching back is just a DOM move.
  getHolder().appendChild(e.el);
  e.host = null;
}

export function fitNow(id: string) {
  const e = pool.get(id);
  if (!e || !e.host) return;
  const { clientWidth, clientHeight } = e.host;
  if (clientWidth < 20 || clientHeight < 20) return;
  try {
    e.fit.fit();
    lastSize = { cols: e.term.cols, rows: e.term.rows };
  } catch {
    /* not measurable yet */
  }
}

export function focus(id: string) {
  pool.get(id)?.term.focus();
}

export function dispose(id: string) {
  const e = pool.get(id);
  if (!e) return;
  e.webgl?.dispose();
  const i = glLru.indexOf(id);
  if (i >= 0) glLru.splice(i, 1);
  e.term.dispose();
  e.el.remove();
  pool.delete(id);
}

/** Re-attach after the backend started a new PTY under the same id (process restart). */
export async function reconnect(id: string, note?: string) {
  const e = pool.get(id);
  if (!e) return;
  if (note) e.term.write(`\r\n\x1b[2m── ${note} ──\x1b[0m\r\n`);
  e.connected = false;
  await connect(id);
}

export function clear(id: string) {
  pool.get(id)?.term.clear();
  void invoke("pty_clear", { id }).catch(() => {});
}

export function find(id: string, q: string, back = false) {
  const e = pool.get(id);
  if (!e || !q) return false;
  const opts = { decorations: { matchOverviewRuler: "#E3B341", activeMatchColorOverviewRuler: "#FFA657", matchBackground: "#4A3B12", activeMatchBackground: "#7A5A12" } };
  return back ? e.search.findPrevious(q, opts) : e.search.findNext(q, opts);
}

export function clearFind(id: string) {
  pool.get(id)?.search.clearDecorations();
}

export function hasSelection(id: string): string | null {
  const e = pool.get(id);
  return e && e.term.hasSelection() ? e.term.getSelection() : null;
}

/** Reset the frontend side after the backend restarted the PTY. */
export function reset(id: string) {
  const e = pool.get(id);
  if (!e) return;
  e.term.reset();
  e.connected = false;
}

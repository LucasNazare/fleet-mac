// Bridges Monaco's built-in LSP client to language servers the Rust backend runs.
import { monaco } from "./monaco";
import { backend, invoke } from "../ipc";
import { ensureModel } from "./models";

type Msg = { jsonrpc: "2.0"; id?: number | string | null; method?: string; params?: any; result?: any; error?: any };

const GROUPS: Record<string, string[]> = {
  typescript: ["typescript", "javascript"],
  python: ["python"],
  go: ["go"],
  rust: ["rust"],
};
export function serverFor(languageId: string): string | null {
  for (const [server, langs] of Object.entries(GROUPS)) if (langs.includes(languageId)) return server;
  return null;
}

const NAV = new Set(["textDocument/definition", "textDocument/typeDefinition", "textDocument/implementation", "textDocument/declaration", "textDocument/references"]);

function uriPath(uri: string) {
  try {
    return decodeURIComponent(uri.replace(/^file:\/\//, ""));
  } catch {
    return uri.replace(/^file:\/\//, "");
  }
}

class TauriTransport {
  private listener: ((m: Msg) => void) | undefined;
  private queue: Msg[] = [];
  private navIds = new Set<number | string>();
  state = { value: { state: "open" as const }, onChange: () => ({ dispose() {} }) };

  constructor(private id: number, private root: string, private languages: string[]) {}

  /** Is this document one of ours (inside the project, right language)? */
  private owns(uri: string | undefined) {
    if (!uri) return true;
    const p = uriPath(uri).toLowerCase();
    if (!p.startsWith(this.root.toLowerCase() + "/")) return false;
    const model = monaco.editor.getModels().find((m) => m.uri.toString(true).toLowerCase() === uri.toLowerCase());
    return !model || this.languages.includes(model.getLanguageId());
  }

  async send(m: Msg) {
    // Monaco's client opens documents with lower-cased URIs but sends later requests with the
    // real case; servers treat those as different files. Always send the model's real URI.
    const td = m.params?.textDocument;
    if (td?.uri) {
      const real = monaco.editor.getModels().find((x) => x.uri.toString(true).toLowerCase() === String(td.uri).toLowerCase());
      if (real) m = { ...m, params: { ...m.params, textDocument: { ...td, uri: real.uri.toString(true) } } };
    }
    const uri = m.params?.textDocument?.uri as string | undefined;
    if (m.method?.startsWith("textDocument/") && !this.owns(uri)) {
      if (m.id != null) this.deliver({ jsonrpc: "2.0", id: m.id, result: null });
      return;
    }
    if (m.id != null && m.method && NAV.has(m.method)) this.navIds.add(m.id);
    await invoke("lsp_send", { id: this.id, message: JSON.stringify(m) });
  }

  setListener(l: ((m: Msg) => void) | undefined) {
    this.listener = l;
    if (l) for (const m of this.queue.splice(0)) l(m);
  }

  toString() {
    return `fleet-lsp#${this.id}`;
  }

  async receive(raw: string) {
    let m: Msg;
    try {
      m = JSON.parse(raw);
    } catch {
      return;
    }
    // Navigation into files that aren't open: create their models first,
    // otherwise Monaco's client can't map the result back.
    if (m.id != null && this.navIds.has(m.id)) {
      this.navIds.delete(m.id);
      const res = Array.isArray(m.result) ? m.result : m.result ? [m.result] : [];
      const uris = new Set<string>(res.map((r: any) => r.targetUri ?? r.uri).filter(Boolean));
      await Promise.all([...uris].map((u) => ensureModel(uriPath(u))));
    }
    this.deliver(m);
  }

  private deliver(m: Msg) {
    if (this.listener) this.listener(m);
    else this.queue.push(m);
  }
}

const clients = new Map<string, Promise<boolean>>();

/** Start (once) the language server for a project + language. */
export function ensureLsp(root: string, languageId: string, custom?: Record<string, string[]>): Promise<boolean> {
  const server = serverFor(languageId);
  if (!server) return Promise.resolve(false);
  const key = `${root}::${server}`;
  if (!clients.has(key)) {
    clients.set(
      key,
      (async () => {
        try {
          const be = await backend();
          if (!be.isTauri) return false;
          const { Channel } = await import("@tauri-apps/api/core");
          const ch = new Channel<string>();
          let transport: TauriTransport | null = null;
          const early: string[] = [];
          ch.onmessage = (raw) => (transport ? void transport.receive(raw) : early.push(raw));
          const id = await invoke<number>("lsp_start", { root, language: server, command: custom?.[server] ?? null, onMessage: ch });
          transport = new TauriTransport(id, root, GROUPS[server]);
          for (const raw of early) void transport.receive(raw);
          new monaco.lsp.MonacoLspClient(transport as never);
          if (server === "typescript") disableBuiltinTs();
          return true;
        } catch (e) {
          console.warn(`[fleet] ${server} language server unavailable:`, e);
          return false;
        }
      })(),
    );
  }
  return clients.get(key)!;
}

let tsDisabled = false;
/** The LSP takes over; Monaco's in-browser TS service would duplicate hovers and errors. */
function disableBuiltinTs() {
  if (tsDisabled) return;
  tsDisabled = true;
  const off = {
    completionItems: false, hovers: false, documentSymbols: false, definitions: false, references: false,
    documentHighlights: false, rename: false, diagnostics: false, documentRangeFormattingEdits: false,
    signatureHelp: false, onTypeFormattingEdits: false, codeActions: false, inlayHints: false,
  };
  const ts = (monaco as any).typescript;
  ts?.typescriptDefaults?.setModeConfiguration?.(off);
  ts?.javascriptDefaults?.setModeConfiguration?.(off);
}

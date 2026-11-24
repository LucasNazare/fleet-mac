// Thin IPC layer. Inside Tauri it talks to the Rust backend; in a plain browser
// (vite dev, Playwright) it falls back to an in-memory mock backend.
import type { Snapshot } from "./types";

export type Unlisten = () => void;
export interface DataChannel {
  onmessage: (data: ArrayBuffer | Uint8Array | string) => void;
}

export interface Backend {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, cb: (payload: T) => void): Promise<Unlisten>;
  channel(): DataChannel & { raw: unknown };
  pickFolder(): Promise<string | null>;
  openUrl(url: string): Promise<void>;
  setTitle(title: string): Promise<void>;
  isTauri: boolean;
}

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriBackend(): Promise<Backend> {
  const core = await import("@tauri-apps/api/core");
  const ev = await import("@tauri-apps/api/event");
  const win = await import("@tauri-apps/api/window");
  const dialog = await import("@tauri-apps/plugin-dialog");
  const opener = await import("@tauri-apps/plugin-opener");
  return {
    isTauri: true,
    invoke: (cmd, args) => core.invoke(cmd, args),
    listen: async (event, cb) => ev.listen(event, (e) => cb(e.payload as never)),
    channel: () => {
      const ch = new core.Channel<ArrayBuffer>();
      const wrapper = { onmessage: (_: ArrayBuffer | Uint8Array | string) => {}, raw: ch };
      ch.onmessage = (d) => wrapper.onmessage(d);
      return wrapper;
    },
    pickFolder: async () => {
      const r = await dialog.open({ directory: true, multiple: false, title: "Add project folder" });
      return typeof r === "string" ? r : null;
    },
    openUrl: (url) => opener.openUrl(url),
    setTitle: (t) => win.getCurrentWindow().setTitle(t),
  };
}

let backendPromise: Promise<Backend> | null = null;
export function backend(): Promise<Backend> {
  if (!backendPromise) {
    backendPromise = isTauri ? tauriBackend() : import("./mockBackend").then((m) => m.createMockBackend());
  }
  return backendPromise;
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return (await backend()).invoke<T>(cmd, args);
}

export type { Snapshot };

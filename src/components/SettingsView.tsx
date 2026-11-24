import { useEffect, useState } from "react";
import { useFleet } from "../store/store";
import { useSettings, DEFAULT_SETTINGS, type Settings } from "../store/settings";
import { COMMANDS, accelOf, clashes, keysFor, prettyKeys, setRecordingShortcut } from "../commands";
import { PALETTE } from "../theme";
import { THEMES, type Effects, type ThemeDef, type ThemeId } from "../themes";
import { invoke } from "../ipc";

type Section = "appearance" | "claude" | "terminal" | "editor" | "git" | "notifications" | "shortcuts";

/** A live miniature of a theme, drawn with its own tokens. */
function ThemeCard({ t, on, onPick }: { t: ThemeDef; on: boolean; onPick: () => void }) {
  const v = t.vars;
  const font = v["font-ui"];
  const r = v.radius;
  return (
    <button className={`theme-card ${on ? "on" : ""}`} onClick={onPick} data-testid="theme-card" data-theme-id={t.id} aria-pressed={on} title={t.blurb}>
      <div className="tc-preview" style={{ background: t.id === "glass" ? "linear-gradient(135deg, #3B4A7A, #7A4A6A 60%, #2A6A7A)" : v["app-bg"], color: v.text, fontFamily: font }}>
        <div className="tc-side" style={{ background: v.panel, borderRight: `1px solid ${v.border}` }}>
          <div className="tc-row" style={{ background: v.sel, borderRadius: r }}><span className="tc-chip" style={{ background: v["lane-2"], color: "#0B0D12", borderRadius: r === "0px" ? 0 : 3 }}>PX</span>Pix fees</div>
          <div className="tc-row" style={{ color: v["state-needs"] }}>● needs you</div>
          <div className="tc-row" style={{ color: v["state-working"] }}>◐ working</div>
        </div>
        <div className="tc-main" style={{ background: t.transparent ? "rgba(10,12,18,.55)" : t.terminal.background, color: t.terminal.foreground, fontFamily: t.mono }}>
          <div className="tc-line"><span style={{ color: t.terminal.green }}>✓</span> tests passed</div>
          <div className="tc-line" style={{ color: t.terminal.brightBlack }}>&gt; ship it_</div>
          <span className="tc-btn" style={{ background: v["primary-bg"], color: v["primary-text"] === "inherit" ? v.text : v["primary-text"], borderRadius: r, border: `1px solid ${v["primary-border"]}`, fontFamily: font }}>Ship</span>
        </div>
      </div>
      <div className="tc-name"><b>{t.name}</b><span>{t.blurb}</span></div>
    </button>
  );
}

function ThemeGallery({ value, onPick }: { value: ThemeId; onPick: (id: ThemeId) => void }) {
  return (
    <div className="theme-grid" role="radiogroup" aria-label="Theme">
      {THEMES.map((t) => <ThemeCard key={t.id} t={t} on={t.id === value} onPick={() => onPick(t.id)} />)}
    </div>
  );
}

function TestNotification() {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const run = async () => {
    setBusy(true);
    try {
      setResult({ ok: true, text: await invoke<string>("notify_test") });
    } catch (e) {
      setResult({ ok: false, text: String(e) });
    }
    setBusy(false);
  };
  return (
    <div className="col">
      <button className="btn small" disabled={busy} onClick={() => void run()} data-testid="notify-test">{busy ? "Sending…" : "Send test notification"}</button>
      {result && <span className={`small ${result.ok ? "muted" : "notify-test-err"}`} data-testid="notify-test-result">{result.text}</span>}
    </div>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="set-row">
      <div><div>{label}</div>{hint && <div className="muted small">{hint}</div>}</div>
      <div className="set-ctl">{children}</div>
    </div>
  );
}

function Num({ value, onChange, min, max, step = 1 }: { value: number; onChange: (n: number) => void; min: number; max: number; step?: number }) {
  return <input type="number" className="field num" value={value} min={min} max={max} step={step} onChange={(e) => { const n = Number(e.target.value); if (!Number.isNaN(n)) onChange(Math.min(max, Math.max(min, n))); }} />;
}

function Toggle({ value, onChange, label }: { value: boolean; onChange: (b: boolean) => void; label?: string }) {
  return <label className="check"><input type="checkbox" checked={value} onChange={(e) => onChange(e.target.checked)} /> {label}</label>;
}

function ShortcutRow({ id, title }: { id: string; title: string }) {
  const update = useSettings((s) => s.update);
  useSettings((s) => s.settings.shortcuts);
  const [recording, setRecording] = useState(false);
  const keys = keysFor(id);
  const clash = keys ? clashes(id, keys) : [];
  // Listen on the window: WebKit doesn't focus buttons on click.
  useEffect(() => {
    if (!recording) return;
    setRecordingShortcut(true);
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") return setRecording(false);
      const a = accelOf(e);
      if (!a) return; // must include ⌘ and no Ctrl
      update((s) => { s.shortcuts[id] = a; });
      setRecording(false);
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      setRecordingShortcut(false);
    };
  }, [recording, id, update]);
  return (
    <div className="set-row">
      <div>
        <div>{title}</div>
        {clash.length > 0 && <div className="clash small" role="alert">⚠ Also used by: {clash.map((c) => c.title).join(", ")}</div>}
      </div>
      <div className="set-ctl">
        <button
          className={`btn small keycap ${recording ? "rec" : ""}`}
          onClick={() => setRecording((r) => !r)}
          data-testid={`shortcut-${id}`}
        >
          {recording ? "Press keys (with ⌘)…" : prettyKeys(keys) || "—"}
        </button>
        <button className="link small" onClick={() => update((s) => { delete s.shortcuts[id]; })}>reset</button>
        <button className="link small" onClick={() => update((s) => { s.shortcuts[id] = ""; })}>none</button>
      </div>
    </div>
  );
}

export function SettingsView() {
  const { settings, update } = useSettings();
  const projects = useFleet((s) => s.projects);
  const [sec, setSec] = useState<Section>("appearance");
  const close = () => useFleet.setState({ settingsOpen: false });
  const set = (fn: (s: Settings) => void) => update(fn);

  return (
    <div className="overlay" onMouseDown={close}>
      <div className="settings" role="dialog" aria-label="Settings" onMouseDown={(e) => e.stopPropagation()} onKeyDown={(e) => { if (e.key === "Escape") close(); }}>
        <nav className="set-nav">
          <h3>Settings</h3>
          {(["appearance", "claude", "terminal", "editor", "git", "notifications", "shortcuts"] as Section[]).map((s) => (
            <button key={s} className={`set-tab ${sec === s ? "on" : ""}`} onClick={() => setSec(s)}>{s[0].toUpperCase() + s.slice(1)}</button>
          ))}
          <span className="spacer" />
          <button className="link small" onClick={() => void invoke("reveal_settings")}>Open settings.json</button>
        </nav>
        <div className="set-body">
          {sec === "appearance" && (
            <>
              <div className="set-row col-row">
                <div><div>Theme</div><div className="muted small">Each theme changes colors, type, shapes, depth and motion — terminals and the editor follow.</div></div>
              </div>
              <ThemeGallery value={settings.appearance.theme} onPick={(id) => set((s) => { s.appearance.theme = id; })} />
              <Row label="Effects" hint="Scanlines, glow, glitches and other motion. Reduced motion in macOS always wins.">
                <select value={settings.appearance.effects ?? "full"} onChange={(e) => set((s) => { s.appearance.effects = e.target.value as Effects; })} data-testid="effects-select">
                  <option value="full">Full</option><option value="subtle">Subtle</option><option value="off">Off</option>
                </select>
              </Row>
              <Row label="UI font size"><Num value={settings.appearance.uiFontSize} min={10} max={18} step={0.5} onChange={(n) => set((s) => { s.appearance.uiFontSize = n; })} /></Row>
              <Row label="Terminal font size"><Num value={settings.appearance.terminalFontSize} min={9} max={24} onChange={(n) => set((s) => { s.appearance.terminalFontSize = n; })} /></Row>
              <Row label="Session colors" hint="Assigned in this order; each meets 4.5:1 contrast on the dark background.">
                <div className="swatches">{settings.appearance.palette.map((c, i) => (
                  <input key={i} type="color" value={c} aria-label={`Color ${i + 1}`} onChange={(e) => set((s) => { s.appearance.palette[i] = e.target.value.toUpperCase(); })} />
                ))}</div>
                <button className="link small" onClick={() => set((s) => { s.appearance.palette = [...PALETTE]; })}>reset</button>
              </Row>
            </>
          )}
          {sec === "claude" && (
            <>
              <Row label="Path to claude" hint="Empty = found on your login shell's PATH."><input className="field" value={settings.claude.path} placeholder="claude" onChange={(e) => set((s) => { s.claude.path = e.target.value; })} /></Row>
              <Row label="Default arguments" hint="Added to every new session, e.g. --model opus"><input className="field" value={settings.claude.defaultArgs} onChange={(e) => set((s) => { s.claude.defaultArgs = e.target.value; })} /></Row>
              <Row label="Default permission mode"><select value={settings.claude.permissionMode} onChange={(e) => set((s) => { s.claude.permissionMode = e.target.value; })}>{["default", "acceptEdits", "plan", "auto"].map((m) => <option key={m}>{m}</option>)}</select></Row>
              <Row label="Model for titles, recaps and commit messages"><input className="field" value={settings.claude.model} onChange={(e) => set((s) => { s.claude.model = e.target.value; })} /></Row>
              <Row label="Live titles" hint="One small claude -p call each time a session finishes a turn."><Toggle value={settings.claude.autoTitles} onChange={(b) => set((s) => { s.claude.autoTitles = b; })} /></Row>
            </>
          )}
          {sec === "terminal" && (
            <>
              <Row label="Scrollback lines"><Num value={settings.terminal.scrollback} min={1000} max={200000} step={1000} onChange={(n) => set((s) => { s.terminal.scrollback = n; })} /></Row>
              <Row label="Cursor"><select value={settings.terminal.cursorStyle} onChange={(e) => set((s) => { s.terminal.cursorStyle = e.target.value as "block"; })}><option>block</option><option>bar</option><option>underline</option></select></Row>
              <Row label="Copy on select"><Toggle value={settings.terminal.copyOnSelect} onChange={(b) => set((s) => { s.terminal.copyOnSelect = b; })} /></Row>
              <Row label="Option key as Meta" hint="Needed for Alt shortcuts in shells and Claude Code."><Toggle value={settings.terminal.optionAsMeta} onChange={(b) => set((s) => { s.terminal.optionAsMeta = b; })} /></Row>
            </>
          )}
          {sec === "editor" && (
            <>
              <Row label="Tab size"><Num value={settings.editor.tabSize} min={1} max={8} onChange={(n) => set((s) => { s.editor.tabSize = n; })} /></Row>
              <Row label="Font size"><Num value={settings.editor.fontSize} min={9} max={24} onChange={(n) => set((s) => { s.editor.fontSize = n; })} /></Row>
              <Row label="Format on save" hint="Uses the project's Prettier, Biome, Black or gofmt when its config exists.">
                <div className="col">{projects.map((p) => (
                  <Toggle key={p.id} label={p.name} value={!!settings.editor.formatOnSave[p.id]} onChange={(b) => set((s) => { s.editor.formatOnSave[p.id] = b; })} />
                ))}</div>
              </Row>
              <Row label="Language servers" hint='JSON: language → command, e.g. {"python": ["pyright-langserver", "--stdio"]}. TypeScript is built in.'>
                <textarea className="field mono" rows={3} defaultValue={JSON.stringify(settings.editor.lspServers, null, 1)} onBlur={(e) => {
                  try { const v = JSON.parse(e.target.value || "{}"); set((s) => { s.editor.lspServers = v; }); } catch { useFleet.getState().showToast("Language servers: not valid JSON", "error"); }
                }} />
              </Row>
            </>
          )}
          {sec === "git" && (
            <>
              <Row label="Show only the session's files by default"><Toggle value={settings.git.onlySessionFiles} onChange={(b) => set((s) => { s.git.onlySessionFiles = b; })} /></Row>
              <Row label="Confirm before discarding" hint="Always on."><Toggle value={true} onChange={() => {}} /></Row>
              <Row label="Auto-fetch every (minutes)" hint="0 = off"><Num value={settings.git.autoFetchMinutes} min={0} max={120} onChange={(n) => set((s) => { s.git.autoFetchMinutes = n; })} /></Row>
            </>
          )}
          {sec === "notifications" && (
            <>
              <Row label="Muted projects"><div className="col">{projects.map((p) => (
                <Toggle key={p.id} label={p.name} value={settings.notifications.mutedProjects.includes(p.id)} onChange={(b) => set((s) => { s.notifications.mutedProjects = b ? [...s.notifications.mutedProjects, p.id] : s.notifications.mutedProjects.filter((x) => x !== p.id); })} />
              ))}</div></Row>
              <Row label="Menu bar item" hint="Lists sessions by state in the macOS menu bar."><Toggle value={settings.notifications.menuBar} onChange={(b) => set((s) => { s.notifications.menuBar = b; })} /></Row>
              <Row label="Sound per state"><Toggle value={settings.notifications.sounds} onChange={(b) => set((s) => { s.notifications.sounds = b; })} /></Row>
              <Row label="Test" hint="Check that macOS shows fleet's notifications (and asks for permission if it hasn't yet)."><TestNotification /></Row>
              <p className="muted small">Mute a single session from its right-click menu.</p>
            </>
          )}
          {sec === "shortcuts" && (
            <>
              <p className="muted small">App shortcuts use ⌘, except ⌃N (session console). Every other Ctrl combination goes to the terminal.</p>
              {COMMANDS.filter((c) => !c.hidden || c.keys).map((c) => <ShortcutRow key={c.id} id={c.id} title={c.title} />)}
              <button className="btn small" onClick={() => update((s) => { s.shortcuts = { ...DEFAULT_SETTINGS.shortcuts }; })}>Reset all shortcuts</button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

import { useState } from "react";
import type { Project, SshConnection } from "../types";
import { useFleet } from "../store/store";
import { invoke } from "../ipc";
import { ContextMenu } from "./ContextMenu";
import { confirmDialog } from "./Dialogs";
import * as pool from "../terminal/termPool";
import { useSettings } from "../store/settings";

const LABEL: Record<string, { sym: string; word: string; color: string }> = {
  connected: { sym: "●", word: "connected", color: "var(--state-done)" },
  connecting: { sym: "◐", word: "connecting…", color: "var(--state-working)" },
  failed: { sym: "✕", word: "failed", color: "var(--state-failed)" },
  disconnected: { sym: "○", word: "disconnected", color: "var(--state-idle)" },
};

export async function connectSsh(conn: SshConnection) {
  const st = useFleet.getState();
  const { cols, rows } = pool.currentSize();
  try {
    await invoke("ssh_connect", { connId: conn.id, cols, rows });
    // Show the login terminal so password / 2FA / host-key prompts can be answered.
    const pty = await invoke<string | null>("ssh_master_pty", { connId: conn.id });
    const s = useFleet.getState().ssh[conn.id];
    if (pty && s?.state === "connecting") {
      useFleet.setState((x) => ({ terminals: { ...x.terminals, [pty]: `Connecting to ${conn.name}` } }));
      st.openProcess(pty);
    }
  } catch (e) {
    st.showToast(String(e), "error");
  }
}

export async function openRemoteTerminal(conn: SshConnection) {
  const st = useFleet.getState();
  const { cols, rows } = pool.currentSize();
  try {
    const id = await invoke<string>("remote_terminal_open", { connId: conn.id, cols, rows });
    useFleet.setState((x) => ({ terminals: { ...x.terminals, [id]: `⇄ ${conn.name}` } }));
    st.openProcess(id);
  } catch (e) {
    st.showToast(String(e), "error");
  }
}

/** Connects if needed (showing the login terminal) and resolves true once connected,
 *  false if the login fails or is cancelled. */
export async function whenConnected(conn: SshConnection): Promise<boolean> {
  const stateOf = () => useFleet.getState().ssh[conn.id]?.state;
  if (stateOf() === "connected") return true;
  if (stateOf() !== "connecting") await connectSsh(conn);
  if (stateOf() === "connected") return true;
  return new Promise((resolve) => {
    let seenConnecting = stateOf() === "connecting";
    const unsub = useFleet.subscribe((s) => {
      const state = s.ssh[conn.id]?.state;
      if (state === "connecting") seenConnecting = true;
      else if (state === "connected") { unsub(); resolve(true); }
      else if (state === "failed" || (state === "disconnected" && seenConnecting)) { unsub(); resolve(false); }
    });
  });
}

/** Click on a Remote row: show its shell (reusing an open one), connecting first if needed. */
export async function openRemote(conn: SshConnection) {
  const st = useFleet.getState();
  const existing = Object.keys(st.terminals).find((id) => id.startsWith(`rterm:${conn.id}:`));
  if (existing && st.ssh[conn.id]?.state === "connected") return st.openProcess(existing);
  if (await whenConnected(conn)) await openRemoteTerminal(conn);
}

/** Starts a claude session that works on the server (connecting first if needed). */
export async function newRemoteSession(projectId: string, conn: SshConnection, prompt = "", permissionMode?: string) {
  if (!(await whenConnected(conn))) return;
  const mode = permissionMode ?? (useSettings.getState().settings.claude.permissionMode || "default");
  await useFleet.getState().newSession(projectId, prompt, mode, pool.currentSize(), false, conn.id);
}

export function RemoteRows({ p }: { p: Project }) {
  const { ssh, setUi, terminals, selectedProcess, centerTab } = useFleet();
  const [menu, setMenu] = useState<{ x: number; y: number; c: SshConnection } | null>(null);
  const conns = p.sshConnections ?? [];

  return (
    <>
      {conns.length > 0 && <div className="subhead">Remote</div>}
      {conns.map((c) => {
        const s = ssh[c.id];
        const state = s?.state ?? "disconnected";
        const l = LABEL[state];
        const up = state === "connected";
        const open = centerTab === "process" && !!selectedProcess && selectedProcess.startsWith(`rterm:${c.id}:`) && !!terminals[selectedProcess];
        return (
          <div key={c.id} className={`remote-row ${open ? "active" : ""}`} onClick={() => void openRemote(c)} onContextMenu={(e) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY, c }); }} data-testid="remote-row" data-state={state} title={`${c.host}:${c.remotePath}${s?.error ? `\n${s.error}` : ""}`}>
            <div className="remote-line">
              <span className="remote-name">⇄ {c.name}</span>
              <span className="remote-state" style={{ color: l.color }}>{l.sym} {l.word}</span>
            </div>
            <div className="remote-line muted small">
              <span className="remote-path">{c.host}:{c.remotePath}{up && s?.sharedWith ? ` · shared with ${s.sharedWith} other project${s.sharedWith === 1 ? "" : "s"}` : ""}</span>
              <span className="remote-actions" onClick={(e) => e.stopPropagation()}>
                {up ? (
                  <button className="link" onClick={() => void invoke("ssh_disconnect", { connId: c.id })}>Disconnect</button>
                ) : state === "connecting" ? (
                  <button className="link" onClick={() => void invoke("ssh_disconnect", { connId: c.id })}>Cancel</button>
                ) : (
                  <button className="link" onClick={() => void connectSsh(c)} data-testid="remote-connect">Connect</button>
                )}
              </span>
            </div>
            {s?.error && state !== "connected" && <div className="remote-err small">{s.error}</div>}
          </div>
        );
      })}
      <button className="ghost-row small" onClick={() => setUi({ sshForm: { projectId: p.id } })} data-testid="add-ssh">+ Add SSH connection</button>
      {menu && (
        <ContextMenu x={menu.x} y={menu.y} onClose={() => setMenu(null)} items={[
          { label: "New claude session", onClick: () => void newRemoteSession(p.id, menu.c) },
          ...(ssh[menu.c.id]?.state === "connected" ? [{ label: "New terminal", onClick: () => void openRemoteTerminal(menu.c) }] : []),
          { label: "Edit…", onClick: () => setUi({ sshForm: { projectId: p.id, conn: menu.c } }) },
          { label: "Show login terminal", onClick: async () => {
            const pty = await invoke<string | null>("ssh_master_pty", { connId: menu.c.id });
            if (pty) { useFleet.setState((x) => ({ terminals: { ...x.terminals, [pty]: `ssh ${menu.c.host}` } })); useFleet.getState().openProcess(pty); }
          } },
          { label: "Remove…", danger: true, onClick: () => confirmDialog({
            title: `Remove ${menu.c.name}?`, body: "Disconnects it from this project. Sessions that used it keep their history.", confirmLabel: "Remove", danger: true,
            onConfirm: () => void invoke("ssh_delete", { projectId: p.id, connId: menu.c.id }),
          }) },
        ]} />
      )}
    </>
  );
}

export function SshForm() {
  const { sshForm, projects, setUi, showToast } = useFleet();
  const c = sshForm?.conn;
  const [name, setName] = useState(c?.name ?? "");
  const [host, setHost] = useState(c?.host ?? "");
  const [port, setPort] = useState(c?.port ? String(c.port) : "");
  const [path, setPath] = useState(c?.remotePath ?? "~/");
  const [identity, setIdentity] = useState(c?.identityFile ?? "");
  const [extra, setExtra] = useState(c?.extraArgs ?? "");
  const [auto, setAuto] = useState(c?.autoConnect ?? false);
  const project = projects.find((p) => p.id === sshForm?.projectId);
  if (!sshForm || !project) return null;
  // Other projects already pointing at the same server (they'll share one connection).
  const same = projects.filter((p) => p.id !== project.id && p.sshConnections?.some((x) => x.host.trim().toLowerCase() === host.trim().toLowerCase() && String(x.port ?? "") === port.trim()));
  const close = () => setUi({ sshForm: null });
  const save = async () => {
    try {
      await invoke("ssh_save", { projectId: project.id, conn: { id: c?.id ?? "", name: name.trim(), host: host.trim(), port: port.trim() ? Number(port) : null, identityFile: identity.trim() || null, remotePath: path.trim() || "~", extraArgs: extra.trim() || null, autoConnect: auto } });
      close();
    } catch (e) {
      showToast(String(e), "error");
    }
  };
  return (
    <div className="overlay" onMouseDown={close}>
      <div className="sheet" role="dialog" aria-label="SSH connection" onMouseDown={(e) => e.stopPropagation()} onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") close(); if (e.key === "Enter" && e.metaKey) void save(); }}>
        <h3>{c ? "Edit" : "Add"} SSH connection · {project.name}</h3>
        <label>Host <span className="muted">(an alias from ~/.ssh/config, or user@host)</span>
          <input className="field" autoFocus value={host} onChange={(e) => setHost(e.target.value)} placeholder="deploy@prod.example.com" data-testid="ssh-host" />
        </label>
        <label>Folder on the server
          <input className="field" value={path} onChange={(e) => setPath(e.target.value)} placeholder="~/apps/atrium" data-testid="ssh-path" />
        </label>
        <label>Name <span className="muted">(optional)</span>
          <input className="field" value={name} onChange={(e) => setName(e.target.value)} placeholder={host || "prod"} />
        </label>
        <div className="row2">
          <label>Port <span className="muted">(optional)</span><input className="field" value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))} placeholder="22" /></label>
          <label>Identity file <span className="muted">(optional)</span><input className="field" value={identity} onChange={(e) => setIdentity(e.target.value)} placeholder="~/.ssh/id_ed25519" /></label>
        </div>
        <label>Extra ssh options <span className="muted">(optional)</span>
          <input className="field" value={extra} onChange={(e) => setExtra(e.target.value)} placeholder="-J bastion" />
        </label>
        <label className="check"><input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} /> Connect when fleet opens</label>
        {same.length > 0 && <p className="muted small">Same server as {same.map((p) => p.name).join(", ")} — fleet will use one shared connection.</p>}
        <p className="muted small">Uses your ssh, ~/.ssh/config, keys and agent. Passwords are typed in fleet's login terminal and never stored. Nothing is installed on the server.</p>
        <div className="sheet-actions">
          <button className="btn" onClick={close}>Cancel</button>
          <button className="btn primary" disabled={!host.trim()} onClick={() => void save()} data-testid="ssh-save">Save ⌘↵</button>
        </div>
      </div>
    </div>
  );
}

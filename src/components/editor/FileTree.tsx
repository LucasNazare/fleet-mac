import { useCallback, useEffect, useState } from "react";
import { useFleet } from "../../store/store";
import { useEditor } from "../../store/editor";
import { useGit, sessionsTouching } from "../../store/git";
import { backend, invoke } from "../../ipc";
import { Chip } from "../common";
import { ContextMenu } from "../ContextMenu";
import { confirmDialog, promptDialog } from "../Dialogs";
import { LETTER_COLOR } from "../git/util";
import type { Project } from "../../types";

interface Entry { name: string; path: string; isDir: boolean; ignored: boolean }

export function FileTree({ project }: { project: Project }) {
  const [children, setChildren] = useState<Record<string, Entry[]>>({});
  const [open, setOpen] = useState<Record<string, boolean>>({ [project.rootPath]: true });
  const [showIgnored, setShowIgnored] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number; e: Entry | null } | null>(null);
  const repo = useGit((g) => g.repos[project.rootPath]);
  const activePath = useEditor((s) => {
    const t = s.tabs.find((x) => x.id === s.active[project.id]);
    return t?.kind === "file" ? t.path : null;
  });
  useFleet((s) => s.sessions);

  const load = useCallback(async (dir: string) => {
    const list = await invoke<Entry[]>("fs_list", { dir, showIgnored }).catch(() => []);
    setChildren((c) => ({ ...c, [dir]: list }));
  }, [showIgnored]);

  useEffect(() => {
    useGit.getState().ensure(project.rootPath);
    for (const d of Object.keys(open).filter((d) => open[d])) void load(d);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project.rootPath, showIgnored]);

  // Refresh expanded folders when files appear / disappear.
  useEffect(() => {
    let un: (() => void) | undefined;
    let t: number | undefined;
    void backend().then((b) => b.listen<{ root: string; paths: string[] }>("fs-changed", (ev) => {
      if (!ev.root.startsWith(project.rootPath)) return;
      window.clearTimeout(t);
      t = window.setTimeout(() => {
        const dirs = new Set(ev.paths.map((p) => p.slice(0, p.lastIndexOf("/"))));
        for (const d of dirs) if (open[d]) void load(d);
      }, 300);
    }).then((u) => (un = u)));
    return () => un?.();
  }, [project.rootPath, open, load]);

  // git status letter per absolute path, and folders that contain changes.
  const statusOf: Record<string, string> = {};
  const top = repo?.status?.top ?? project.rootPath;
  for (const f of repo?.status?.status?.files ?? []) {
    const abs = `${top}/${f.path}`;
    statusOf[abs] = f.conflicted ? "U" : f.untracked ? "?" : f.worktree !== "." ? f.worktree : f.index;
    let d = abs.slice(0, abs.lastIndexOf("/"));
    while (d.length > project.rootPath.length) {
      statusOf[d] ??= "•";
      d = d.slice(0, d.lastIndexOf("/"));
    }
  }

  const toggle = (e: Entry) => {
    const next = !open[e.path];
    setOpen((o) => ({ ...o, [e.path]: next }));
    if (next && !children[e.path]) void load(e.path);
  };
  const openFile = (e: Entry) => useEditor.getState().open({ kind: "file", projectId: project.id, path: e.path });
  const parentOf = (p: string) => p.slice(0, p.lastIndexOf("/"));

  const create = (dirPath: string, dir: boolean) =>
    promptDialog({
      title: dir ? "New folder" : "New file",
      label: `In ${dirPath.slice(project.rootPath.length) || "/"}`,
      placeholder: dir ? "folder" : "file.ts",
      confirmLabel: "Create",
      onSubmit: async (name) => {
        const p = `${dirPath}/${name}`;
        try {
          await invoke("fs_create", { path: p, dir });
          setOpen((o) => ({ ...o, [dirPath]: true }));
          await load(dirPath);
          if (!dir) useEditor.getState().open({ kind: "file", projectId: project.id, path: p });
        } catch (err) { useFleet.getState().showToast(String(err), "error"); }
      },
    });

  const items = (e: Entry | null) => {
    const dirPath = e ? (e.isDir ? e.path : parentOf(e.path)) : project.rootPath;
    const base = [
      { label: "New file…", onClick: () => create(dirPath, false) },
      { label: "New folder…", onClick: () => create(dirPath, true) },
    ];
    if (!e) return base;
    return [
      ...base,
      { label: "Rename…", onClick: () => promptDialog({ title: "Rename", initial: e.name, confirmLabel: "Rename", onSubmit: async (n) => {
        try { await invoke("fs_rename", { from: e.path, to: `${parentOf(e.path)}/${n}` }); await load(parentOf(e.path)); } catch (err) { useFleet.getState().showToast(String(err), "error"); }
      } }) },
      { label: "Move to Trash…", danger: true, onClick: () => confirmDialog({ title: `Move ${e.name} to the Trash?`, confirmLabel: "Move to Trash", danger: true, onConfirm: async () => {
        await invoke("fs_trash", { path: e.path }).catch((err) => useFleet.getState().showToast(String(err), "error"));
        await load(parentOf(e.path));
      } }) },
      { label: "Reveal in Finder", onClick: () => void invoke("fs_reveal", { path: e.path }) },
      { label: "Copy path", onClick: () => void navigator.clipboard.writeText(e.path) },
      { label: "Copy relative path", onClick: () => void navigator.clipboard.writeText(e.path.slice(project.rootPath.length + 1)) },
      { label: "Send path to session…", onClick: () => useFleet.setState({ sendPicker: { text: e.path.slice(project.rootPath.length + 1), header: "File:", projectId: project.id } }) },
    ];
  };

  const render = (dir: string, depth: number): React.ReactNode =>
    (children[dir] ?? []).map((e) => {
      const st = statusOf[e.path];
      const chips = e.isDir ? [] : sessionsTouching(e.path).filter((s) => s.alive);
      return (
        <div key={e.path}>
          <div
            className={`tree-row ${activePath === e.path ? "on" : ""} ${e.ignored ? "ignored" : ""}`}
            style={{ paddingLeft: 10 + depth * 12 }}
            onClick={() => (e.isDir ? toggle(e) : openFile(e))}
            onContextMenu={(ev) => { ev.preventDefault(); ev.stopPropagation(); setMenu({ x: ev.clientX, y: ev.clientY, e }); }}
            title={e.path}
            data-testid="tree-row"
          >
            <span className="caret">{e.isDir ? (open[e.path] ? "▾" : "▸") : ""}</span>
            <span className="tree-name" style={st && st !== "•" ? { color: LETTER_COLOR[st] ?? undefined } : undefined}>{e.name}</span>
            {chips.map((s) => <Chip key={s.id} s={s} size={15} />)}
            {st && <span className="tree-st" style={{ color: LETTER_COLOR[st] ?? "var(--muted)" }}>{st === "•" ? "•" : st}</span>}
          </div>
          {e.isDir && open[e.path] && render(e.path, depth + 1)}
        </div>
      );
    });

  return (
    <div className="file-tree" onContextMenu={(ev) => { ev.preventDefault(); setMenu({ x: ev.clientX, y: ev.clientY, e: null }); }} data-testid="file-tree">
      <div className="tree-head">
        <select className="proj-select tree-proj" value={project.id} onChange={(e) => useFleet.getState().setUi({ editorProjectId: e.target.value })} aria-label="Project" data-testid="tree-project">
          {useFleet.getState().projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
        <span className="spacer" />
        <button className="icon-btn" title="New file" aria-label="New file" onClick={() => create(project.rootPath, false)}>+</button>
        <label className="check small" title="Show files ignored by .gitignore"><input type="checkbox" checked={showIgnored} onChange={(e) => setShowIgnored(e.target.checked)} /> ignored</label>
      </div>
      <div className="tree-body">{render(project.rootPath, 0)}</div>
      {menu && <ContextMenu x={menu.x} y={menu.y} onClose={() => setMenu(null)} items={items(menu.e)} />}
    </div>
  );
}

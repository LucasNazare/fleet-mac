import { useState } from "react";
import { useFleet } from "../../store/store";
import { useGit, joinPath, sessionsTouching } from "../../store/git";
import { useEditor } from "../../store/editor";
import { invoke } from "../../ipc";
import type { FileEntry, Session } from "../../types";
import { Chip } from "../common";
import { confirmDialog } from "../Dialogs";
import { ContextMenu } from "../ContextMenu";
import { Counts, LETTER_COLOR, letterFor, splitPath } from "./util";
import { openSendPicker } from "../SendPicker";

function Row({ f, staged, base, root, onOpen }: { f: FileEntry; staged: boolean; base: string; root: string; onOpen: () => void }) {
  const act = useGit((g) => g.act);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const letter = letterFor(f, staged);
  const { dir, name } = splitPath(f.path);
  const touching = sessionsTouching(joinPath(base, f.path));
  const discard = () =>
    confirmDialog({
      title: "Discard changes?",
      body: <>This throws away your uncommitted changes to <b>{f.path}</b>{f.untracked ? " (the file goes to the Trash)" : ""}. It can't be undone.</>,
      confirmLabel: "Discard",
      danger: true,
      onConfirm: () => void act(root, "discard", () => invoke("git_discard", { root, paths: [f.path] })),
    });
  return (
    <div className={`git-row ${f.conflicted ? "conflict" : ""}`} onClick={onOpen} onContextMenu={(e) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY }); }}
      title={f.origPath ? `${f.origPath} → ${f.path}` : f.path} data-testid={staged ? "staged-row" : "change-row"}>
      <span className="letter" style={{ color: LETTER_COLOR[letter] ?? "var(--muted)" }}>{letter}</span>
      <span className="gpath"><span className="gname">{name}</span> <span className="muted gdir">{dir}</span></span>
      <span className="chips">{touching.map((s) => <Chip key={s.id} s={s} size={16} />)}</span>
      <Counts a={staged ? f.stagedAdded : f.added} r={staged ? f.stagedRemoved : f.removed} />
      <span className="row-actions">
        {staged ? (
          <button title="Unstage" aria-label={`Unstage ${f.path}`} onClick={(e) => { e.stopPropagation(); void act(root, "unstage", () => invoke("git_unstage", { root, paths: [f.path] })); }}>−</button>
        ) : (
          <>
            {!f.conflicted && <button title="Discard changes" aria-label={`Discard ${f.path}`} onClick={(e) => { e.stopPropagation(); discard(); }}>↺</button>}
            <button title={f.conflicted ? "Mark resolved (stage)" : "Stage"} aria-label={`Stage ${f.path}`} onClick={(e) => { e.stopPropagation(); void act(root, "stage", () => invoke("git_stage", { root, paths: [f.path] })); }} data-testid="stage-btn">+</button>
          </>
        )}
      </span>
      {menu && (
        <ContextMenu x={menu.x} y={menu.y} onClose={() => setMenu(null)} items={[
          { label: "Open diff", onClick: onOpen },
          { label: "Open file", onClick: () => useEditor.getState().open({ kind: "file", projectId: useFleet.getState().editorProjectId ?? "", path: joinPath(base, f.path) }) },
          staged ? { label: "Unstage", onClick: () => void act(root, "unstage", () => invoke("git_unstage", { root, paths: [f.path] })) }
            : { label: "Stage", onClick: () => void act(root, "stage", () => invoke("git_stage", { root, paths: [f.path] })) },
          ...(staged ? [] : [{ label: "Discard changes…", danger: true, onClick: discard }]),
          { label: "Copy path", onClick: () => void navigator.clipboard.writeText(joinPath(base, f.path)) },
        ]} />
      )}
    </div>
  );
}

export function ChangesTab({ root }: { root: string }) {
  const { repos, act, onlySessionFiles, set } = useGit();
  const { sessions, activeSessionId, projects } = useFleet();
  const [msg, setMsg] = useState("");
  const [amend, setAmend] = useState(false);
  const [drafting, setDrafting] = useState(false);
  const [commitMenu, setCommitMenu] = useState(false);
  const repo = repos[root];
  const st = repo?.status?.status;
  const base = repo?.status?.top ?? root;
  const active: Session | null = activeSessionId ? sessions[activeSessionId] : null;
  if (!st) return <div className="muted pad">Loading…</div>;

  const mine = (f: FileEntry) => !onlySessionFiles || !active || active.filesTouched.includes(joinPath(base, f.path));
  const conflicts = st.files.filter((f) => f.conflicted);
  const staged = st.files.filter((f) => !f.conflicted && f.index !== "." && f.index !== "?").filter(mine);
  const changes = st.files.filter((f) => !f.conflicted && (f.worktree !== "." || f.untracked)).filter(mine);
  const projectId = projects.find((p) => base.startsWith(p.rootPath))?.id ?? useFleet.getState().editorProjectId ?? "";

  // Files changed here that more than one live session has touched.
  const overlaps = st.files
    .map((f) => ({ f, ss: sessionsTouching(joinPath(base, f.path)).filter((s) => s.alive) }))
    .filter((x) => x.ss.length > 1);

  const openDiff = (f: FileEntry, stagedSide: boolean) => {
    const { open } = useEditor.getState();
    if (f.conflicted) open({ kind: "conflict", projectId, root: base, path: f.path });
    else open({ kind: "diff", projectId, root: base, path: f.path, staged: stagedSide });
  };

  const commit = async (push: boolean) => {
    const ok = await act(root, push ? "push" : "commit", () => invoke("git_commit", { root, message: msg, amend, push }));
    if (ok) { setMsg(""); setAmend(false); }
  };
  const draft = async () => {
    setDrafting(true);
    try {
      const m = await invoke<string>("git_write_message", { root, model: null });
      setMsg(m);
    } catch (e) {
      useGit.setState((g) => ({ repos: { ...g.repos, [root]: { ...g.repos[root], error: e as never } } }));
    }
    setDrafting(false);
  };

  return (
    <div className="changes">
      <div className="filter-row">
        <select value={onlySessionFiles ? "mine" : "all"} onChange={(e) => set({ onlySessionFiles: e.target.value === "mine" })} aria-label="Filter changes">
          <option value="all">All changes</option>
          <option value="mine" disabled={!active}>Only {active ? `${active.glyph}'s` : "this session's"} files</option>
        </select>
      </div>
      {overlaps.length > 0 && (
        <div className="overlap" role="alert" data-testid="overlap">
          {overlaps.slice(0, 4).map(({ f, ss }) => (
            <div key={f.path}>⚠ <b>{splitPath(f.path).name}</b> is being changed by {ss.map((s) => `${s.glyph} · ${s.title}`).join(" and ")}</div>
          ))}
          {overlaps.length > 4 && <div className="muted">…and {overlaps.length - 4} more</div>}
        </div>
      )}
      {conflicts.length > 0 && (
        <section>
          <div className="git-group"><span>Merge conflicts</span><span className="muted">{conflicts.length}</span>
            <span className="spacer" />
            <button className="link" onClick={() => {
              const st0 = useFleet.getState();
              useFleet.setState({ sendPicker: { text: `Please resolve the merge conflicts in:\n${conflicts.map((c) => `- ${c.path}`).join("\n")}\nThen stage the files.`, header: `From the git panel (${base}):`, projectId } });
              void st0;
            }}>Ask a session to resolve</button>
          </div>
          {conflicts.map((f) => <Row key={f.path} f={f} staged={false} base={base} root={root} onOpen={() => openDiff(f, false)} />)}
        </section>
      )}
      <section>
        <div className="git-group">
          <span>Staged</span><span className="muted">{staged.length}</span><span className="spacer" />
          {staged.length > 0 && <button className="link" onClick={() => void act(root, "unstage", () => invoke("git_unstage", { root, paths: staged.map((f) => f.path) }))}>unstage all</button>}
        </div>
        {staged.map((f) => <Row key={f.path} f={f} staged base={base} root={root} onOpen={() => openDiff(f, true)} />)}
      </section>
      <section>
        <div className="git-group">
          <span>Changes</span><span className="muted">{changes.length}</span><span className="spacer" />
          {changes.length > 0 && <button className="link" onClick={() => void act(root, "stage", () => invoke("git_stage", { root, paths: changes.map((f) => f.path) }))} data-testid="stage-all">stage all</button>}
        </div>
        {changes.map((f) => <Row key={f.path} f={f} staged={false} base={base} root={root} onOpen={() => openDiff(f, false)} />)}
        {st.files.length === 0 && <div className="muted pad">Working tree clean.</div>}
      </section>
      <div className="commit-box">
        <textarea
          value={msg}
          rows={3}
          placeholder={`Message (⌘↵ to commit on ${st.branch})`}
          onChange={(e) => setMsg(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && e.metaKey) { e.preventDefault(); e.stopPropagation(); void commit(e.shiftKey); } }}
          data-testid="commit-msg"
        />
        <div className="commit-actions">
          <button className="btn small" onClick={() => void draft()} disabled={drafting || staged.length === 0} title="Draft a message from the staged diff (claude -p --model haiku)" data-testid="write-with-claude">
            {drafting ? "Writing…" : "✦ Write with Claude"}
          </button>
          <label className="check small"><input type="checkbox" checked={amend} onChange={(e) => setAmend(e.target.checked)} /> Amend</label>
          <span className="spacer" />
          <div className="split-btn">
            <button className="btn small primary" disabled={(!msg.trim() && !amend) || (staged.length === 0 && !amend) || !!repo?.busy} onClick={() => void commit(false)} data-testid="commit-btn">
              {repo?.busy === "commit" ? "Committing…" : "Commit ⌘↵"}
            </button>
            <button className="btn small primary caret" aria-label="More commit options" onClick={() => setCommitMenu((v) => !v)}>▾</button>
            {commitMenu && (
              <div className="menu commit-menu" onMouseLeave={() => setCommitMenu(false)}>
                <button className="menu-item" disabled={(!msg.trim() && !amend) || staged.length === 0} onClick={() => { setCommitMenu(false); void commit(true); }} data-testid="commit-push">Commit and push ⌘⇧↵</button>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

export { openSendPicker };

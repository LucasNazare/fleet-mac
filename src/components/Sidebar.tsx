import { memo, useState } from "react";
import { useFleet, orderedProjects, projectSessions, matchesFilter } from "../store/store";
import { needsYou, STATE } from "../theme";
import { SessionRow } from "./SessionRow";
import { useNow } from "./common";
import { ContextMenu } from "./ContextMenu";
import { invoke } from "../ipc";
import { ProcessRows } from "./ProcessRows";
import { RemoteRows } from "./RemoteRows";
import { FileTree } from "./editor/FileTree";
import { FindInFiles } from "./editor/FindInFiles";
import { editorProject } from "./editor/QuickOpen";
import { ProjectSyncPill, LocalWorkBanner } from "./SyncUi";
import { OtherComputers } from "./ShipUi";
import { useSync } from "../store/sync";

function ModeTabs() {
  const { sidebarMode, setUi } = useFleet();
  return (
    <nav className="side-modes">
      {(["sessions", "files", "search"] as const).map((m) => (
        <button key={m} className={`tab ${sidebarMode === m ? "on" : ""}`} onClick={() => setUi({ sidebarMode: m })} data-testid={`mode-${m}`}>
          {m === "sessions" ? "Sessions" : m === "files" ? "Files" : "Search"}
        </button>
      ))}
    </nav>
  );
}

export const Sidebar = memo(function Sidebar({ searchRef }: { searchRef: React.RefObject<HTMLInputElement | null> }) {
  const mode = useFleet((s) => s.sidebarMode);
  useFleet((s) => s.editorProjectId);
  useFleet((s) => s.activeSessionId);
  if (mode !== "sessions") {
    const project = editorProject();
    return (
      <aside className="sidebar">
        <ModeTabs />
        {!project ? <div className="empty-side">Add a project first.</div> : mode === "files" ? <FileTree key={project.id} project={project} /> : <FindInFiles key={project.id} project={project} />}
      </aside>
    );
  }
  return <SessionSidebar searchRef={searchRef} />;
});

function SessionSidebar({ searchRef }: { searchRef: React.RefObject<HTMLInputElement | null> }) {
  const { projects, sessions, filter, search, setUi, addProject } = useFleet();
  const now = useNow();
  const [menu, setMenu] = useState<{ x: number; y: number; id: string } | null>(null);
  const [showArchived, setShowArchived] = useState<Record<string, boolean>>({});

  return (
    <aside className="sidebar">
      <ModeTabs />
      <div className="search-wrap">
        <input
          ref={searchRef}
          className="search"
          placeholder="Filter sessions"
          value={search}
          onChange={(e) => setUi({ search: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Escape") { setUi({ search: "" }); (e.target as HTMLInputElement).blur(); }
            if (e.key === "Enter") {
              const first = orderedProjects(projects).flatMap((p) => projectSessions(p.id, sessions)).find((s) => matchesFilter(s, filter, search));
              if (first) { useFleet.getState().select(first.id); setUi({ search: "" }); }
            }
          }}
        />
        {filter && (
          <button className="filter-pill" onClick={() => setUi({ filter: null })} title="Clear filter">
            {filter === "needs" ? "needs you" : filter === "done" ? "to review" : filter} ✕
          </button>
        )}
      </div>
      <div className="groups">
        {projects.length === 0 && (
          <div className="empty-side">
            <p>No projects yet.</p>
            <button className="btn primary" onClick={() => void addProject()}>Add a project folder</button>
          </div>
        )}
        {orderedProjects(projects).map((p) => {
          const all = projectSessions(p.id, sessions);
          const shown = all.filter((s) => matchesFilter(s, filter, search));
          const archived = projectSessions(p.id, sessions, true);
          const attention = all.some((s) => needsYou(s.state));
          if ((filter || search) && shown.length === 0) return null;
          return (
            <section key={p.id} className="group" data-testid="project-group">
              <div
                className="group-header"
                onClick={() => void invoke("set_project_collapsed", { id: p.id, collapsed: !p.collapsed })}
                onContextMenu={(e) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY, id: p.id }); }}
                title={p.rootPath}
              >
                <span className="caret">{p.collapsed ? "▸" : "▾"}</span>
                <span className="group-name">{p.name}</span>
                <ProjectSyncPill projectId={p.id} />
                {attention && <span className="attn" title="A session here needs you" style={{ color: STATE.needs_reply.color }}>●</span>}
                <span className="muted small">{all.length}</span>
                <span className="spacer" />
                <button
                  className="icon-btn"
                  title={`New session in ${p.name}`}
                  aria-label={`New session in ${p.name}`}
                  onClick={(e) => { e.stopPropagation(); void useFleet.getState().quickSession(p.id); }}
                >
                  +
                </button>
              </div>
              {!p.collapsed && (
                <>
                  {!filter && !search && <LocalWorkBanner projectId={p.id} />}
                  {shown.length > 0 && <div className="subhead">Sessions</div>}
                  {shown.map((s) => <SessionRow key={s.id} s={s} now={now} />)}
                  {all.length === 0 && (
                    <button className="ghost-row" onClick={() => void useFleet.getState().quickSession(p.id)}>
                      + Start a Claude Code session
                    </button>
                  )}
                  {!filter && !search && <OtherComputers projectId={p.id} />}
                  {!filter && !search && <ProcessRows p={p} now={now} />}
                  {!filter && !search && <RemoteRows p={p} />}
                  {archived.length > 0 && !filter && !search && (
                    <>
                      <button className="subhead link" onClick={() => setShowArchived((a) => ({ ...a, [p.id]: !a[p.id] }))}>
                        {showArchived[p.id] ? "▾" : "▸"} Archived ({archived.length})
                      </button>
                      {showArchived[p.id] && archived.map((s) => <SessionRow key={s.id} s={s} now={now} />)}
                    </>
                  )}
                </>
              )}
            </section>
          );
        })}
      </div>
      <div className="sidebar-foot">
        <button className="btn" onClick={() => void addProject()} data-testid="add-project">+ Add project</button>
        <button className="btn" onClick={() => useSync.setState({ reposOpen: true })} data-testid="add-from-github" title="Clone one of your GitHub repos">+ From GitHub</button>
      </div>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={[
            { label: "New session", onClick: () => void useFleet.getState().quickSession(menu.id) },
            { label: "New session with prompt / worktree…", onClick: () => setUi({ sheetOpen: true, sheetProjectId: menu.id }) },
            ...(() => {
              const c = useSync.getState().configs[menu.id];
              const p = projects.find((x) => x.id === menu.id);
              if (!c?.enabled) return [{ label: "GitHub sync…", onClick: () => useSync.setState({ setup: menu.id }) }];
              return [
                { label: "Edit fleet.yml…", onClick: () => void useSync.getState().openEditor(menu.id) },
                { label: p?.syncPaused ? "Resume GitHub sync" : "Pause GitHub sync", onClick: () => void invoke("sync_set_paused", { projectId: menu.id, paused: !p?.syncPaused }) },
                { label: "GitHub sync status…", onClick: () => useSync.setState({ setup: menu.id }) },
              ];
            })(),
            { label: "Remove project from fleet", danger: true, onClick: () => void invoke("remove_project", { id: menu.id }) },
          ]}
        />
      )}
    </aside>
  );
}

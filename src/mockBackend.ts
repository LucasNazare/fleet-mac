// In-memory stand-in for the Rust backend, used in a plain browser and by Playwright.
// window.__fleetMock exposes hooks so tests can drive state changes.
import type { Backend, DataChannel } from "./ipc";
import type { ProcessDef, ProcessRun, Project, Session, SessionMetrics, SessionState, UsageDay } from "./types";
import { procKey } from "./types";
import { PALETTE } from "./theme";

type Listener = (p: unknown) => void;

export function createMockBackend(): Backend {
  const listeners = new Map<string, Set<Listener>>();
  const emit = (event: string, payload: unknown) => listeners.get(event)?.forEach((l) => l(payload));
  const projects: Project[] = [];
  const sessions: Session[] = [];
  const channels = new Map<string, DataChannel>();
  const enc = new TextEncoder();
  let folderN = 0;
  const runs = new Map<string, ProcessRun>();
  const sent: { sessionId: string; text: string }[] = [];
  /** Everything typed into terminals (pty_write), in order. */
  const ptyWrites: { id: string; data: string }[] = [];
  const emitRun = (r: ProcessRun) => emit("process-updated", { ...r });
  // ---- tiny in-memory git repo + file system (same files in every project root)
  const repo = {
    files: {
      "src/fee.ts": { head: "export const fee = 1;\nexport const label = 'Pix';\n", index: "export const fee = 1;\nexport const label = 'Pix';\n", work: "export const fee = 2;\nexport const label = 'Pix fee';\n" },
      "README.md": { head: "# demo\n", index: "# demo\n", work: "# demo\n" },
      "src/new.ts": { head: null as string | null, index: null as string | null, work: "export {};\n" },
    } as Record<string, { head: string | null; index: string | null; work: string | null }>,
    branch: "main",
    upstream: "origin/main" as string | null,
    ahead: 0,
    log: [{ hash: "a1b2c3d4e5f6", parents: [] as string[], author: "Dev", time: Math.floor(Date.now() / 1000) - 3600, subject: "Initial commit", refs: "HEAD -> main" }],
    stashes: [] as { name: string; message: string; time: number }[],
  };
  let outN = 0;
  const gitOut = (root: string, cmd: string, output = "", code = 0, auto = false) => {
    emit("git-output", { id: outN++, root, cmd, output, code, ms: 3, at: Date.now(), auto });
    emit("git-changed", root);
  };
  const relOf = (path: string) => path.replace(/^\/Users\/dev\/code\/project-\d+\//, "");
  const statusOf = () => ({
    branch: repo.branch, oid: repo.log[0].hash, upstream: repo.upstream, ahead: repo.ahead, behind: 0,
    files: Object.entries(repo.files).flatMap(([path, f]) => {
      const idx = f.index === f.head ? "." : f.head == null ? "A" : f.index == null ? "D" : "M";
      const wt = f.index == null && f.head == null ? "?" : f.work === f.index ? "." : f.work == null ? "D" : "M";
      if (idx === "." && wt === ".") return [];
      return [{ path, index: idx, worktree: wt, conflicted: false, untracked: wt === "?", added: 1, removed: 1, stagedAdded: idx === "." ? null : 1, stagedRemoved: idx === "." ? null : 1 }];
    }),
  });
  const gitErr = (message: string, fix?: string, fixLabel?: string) => ({ message, fix: fix ?? null, fixLabel: fixLabel ?? null, raw: message });

  let termN = 0;
  const sshWanted = new Set<string>();
  const sshServers = new Set<string>();
  const sshConnects: string[] = [];
  // ---- GitHub sync (mock): configs per project, a fake remote that can go offline
  const syncCfg: Record<string, any> = {};
  const syncStatus = { lastSyncAt: Date.now() as number | null, offline: false, waiting: 0, pausedAll: false, anySynced: false };
  const remoteBranches = new Set<string>();
  let syncOffline = false;
  const emitSync = () => {
    syncStatus.anySynced = Object.values(syncCfg).some((c) => c.enabled);
    syncStatus.waiting = sessions.filter((s) => s.sync?.badge === "waiting").length;
    syncStatus.offline = syncOffline;
    emit("sync-configs", Object.values(syncCfg));
    emit("sync-status", { ...syncStatus });
  };
  const mockValidate = (text: string) => {
    const errs: { line: number; message: string }[] = [];
    text.split("\n").forEach((l, i) => {
      const m = /^\s*(\w+):/.exec(l);
      if (m && ["enabeld", "proccesses", "deploycommand"].includes(m[1])) errs.push({ line: i + 1, message: `Additional properties are not allowed ('${m[1]}' was unexpected)` });
      if (/fetchIntervalSec:\s*[a-z]/.test(l)) errs.push({ line: i + 1, message: `sync.fetchIntervalSec: "${l.split(":")[1].trim()}" is not of type "integer"` });
    });
    if (!/^project:/m.test(text) && !errs.length && !text.trim()) errs.push({ line: 1, message: "fleet.yml is empty" });
    return errs;
  };
  const slug = (t: string) => t.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 40) || "session";
  const syncSession = (s: Session) => {
    const p = projects.find((x) => x.id === s.projectId)!;
    if (!syncCfg[p.id]?.enabled || s.remote) return;
    if (p.syncPaused || syncStatus.pausedAll) { s.sync = { badge: "paused", waiting: 0, excluded: [], detail: "Sync is paused" }; emit("session-updated", { ...s }); return; }
    if (!s.branch || s.branch === "main") s.branch = `fleet/${s.glyph.toLowerCase()}-${slug(s.title)}`;
    s.promptCount = (s.promptCount ?? 0) + 1;
    const root = s.worktreePath ?? p.rootPath;
    gitOut(root, "git add -A", "", 0, true);
    gitOut(root, "git commit -q --no-verify -F -", "", 0, true);
    if (syncOffline) {
      gitOut(root, `git push --porcelain --atomic origin refs/heads/${s.branch}:refs/heads/${s.branch} refs/heads/fleet-state:refs/heads/fleet-state`, "fatal: unable to access 'https://github.com/dev/demo.git/': Could not resolve host: github.com", 128, true);
      s.sync = { badge: "waiting", waiting: 1, excluded: [], detail: "Could not resolve host: github.com" };
    } else {
      gitOut(root, `git push --porcelain --atomic origin refs/heads/${s.branch}:refs/heads/${s.branch} refs/heads/fleet-state:refs/heads/fleet-state`, "", 0, true);
      remoteBranches.add(s.branch);
      s.sync = { badge: "pushed", waiting: 0, excluded: s.sync?.excluded ?? [], lastPushAt: Date.now(), detail: null };
      syncStatus.lastSyncAt = Date.now();
    }
    emit("session-updated", { ...s });
    emitSync();
  };
  // Stage B/C mock state
  let others: any[] = [];
  const deploys: any[] = [];
  const shipConflicts = new Set<string>();
  /** Projects with no origin remote (GitHub sync → Create GitHub repo). */
  const noOrigin = new Set<string>();
  let localWork: Record<string, number> = {};
  const findDef = (projectId: string, defId: string) => projects.find((p) => p.id === projectId)?.processes.find((d) => d.id === defId);

  const write = (id: string, s: string) => channels.get(id)?.onmessage(enc.encode(s));
  const glyphFor = (t: string) => {
    const taken = new Set(sessions.map((s) => s.glyph));
    const letters = t.toUpperCase().replace(/[^A-Z0-9]/g, "") + "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    for (let i = 1; i < letters.length; i++) {
      const g = letters[0] + letters[i];
      if (!taken.has(g)) return g;
    }
    return "??";
  };
  const colorFor = () => PALETTE.find((c) => !sessions.some((s) => !s.archived && s.color === c)) ?? PALETTE[0];

  const handlers: Record<string, (a: any) => unknown> = {
    usage_report: ({ days }) => {
      const out: UsageDay[] = [];
      const day = (ago: number) => { const d = new Date(Date.now() - ago * 86_400_000); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`; };
      for (let i = 0; i < Math.min(days as number, 3); i++) {
        for (const [n, p] of projects.entries()) {
          out.push({ day: day(i), project: p.id, cost: 1.5 / (n + 1), subCost: 0.5 / (n + 1), tokens: 900_000, subTokens: 300_000, turns: 4, turnMs: 480_000, waitMs: 60_000, maxTurnMs: 240_000 });
        }
      }
      return out;
    },
    get_snapshot: () => ({ projects, sessions, ui: null, restored: 0, warnings: [], runs: [...runs.values()] }),
    add_project: ({ path }) => {
      const p: Project = {
        id: crypto.randomUUID(), name: path.split("/").pop(), rootPath: path, colorFamily: PALETTE[projects.length % 12],
        processes: [], defaultClaudeArgs: [], collapsed: false, lastActivity: Date.now(), processesOffered: false, sshConnections: [],
      };
      projects.push(p);
      emit("projects-changed", projects);
      return p;
    },
    remove_project: ({ id }) => {
      projects.splice(projects.findIndex((p) => p.id === id), 1);
      emit("projects-changed", projects);
    },
    set_project_collapsed: ({ id, collapsed }) => {
      const p = projects.find((p) => p.id === id);
      if (p) p.collapsed = collapsed;
      emit("projects-changed", projects);
    },
    new_session: ({ projectId, prompt, permissionMode, remote }) => {
      const project = projects.find((p) => p.id === projectId)!;
      if (remote) {
        const conn = project.sshConnections.find((c) => c.id === remote);
        if (!conn) throw "unknown SSH connection";
        if (!sshWanted.has(remote) || !sshServers.has(conn.host.toLowerCase())) throw "Connect to the server first.";
      }
      const title = prompt?.trim() ? prompt.trim().split(/\s+/).slice(0, 6).join(" ") : "New session";
      const s: Session = {
        id: crypto.randomUUID(), projectId, claudeSessionId: crypto.randomUUID(),
        glyph: glyphFor(title === "New session" ? project.name : title), color: colorFor(), title,
        titleHistory: [], recap: [], cwd: project.rootPath, branch: "main", state: prompt ? "working" : "idle",
        stateSince: Date.now(), filesTouched: [], createdAt: Date.now(), pinned: false, pinnedColor: false,
        pinnedGlyph: false, titleLocked: false, archived: false, permissionMode, alive: true, contextPct: 12, remote: remote ?? null,
      };
      if (syncCfg[projectId]?.enabled && !remote) {
        s.worktreePath = `${project.rootPath}/.fleet/worktrees/${slug(title === "New session" ? "session" : title)}-${sessions.length}`;
        s.branch = "";
        s.sync = { badge: "new", waiting: 0, excluded: [], detail: "Gets its branch when the first prompt finishes" };
      }
      sessions.push(s);
      emit("session-updated", s);
      return s;
    },
    pty_attach: ({ id, onData }) => {
      channels.set(id, onData as DataChannel);
      if (id.startsWith("proc:")) return runs.has(id);
      if (id.startsWith("console:") || id.startsWith("rterm:")) { write(id, "$ "); return true; }
      const s = sessions.find((s) => s.id === id);
      write(id, `\x1b[38;2;255;166;87m✻\x1b[0m mock claude · ${s?.title}\r\n\r\n> `);
      return true;
    },
    pty_write: ({ id, data }) => {
      ptyWrites.push({ id, data });
      write(id, data === "\r" ? "\r\n> " : data);
    },
    pty_resize: () => {},
    restart_session: ({ id }) => {
      const s = sessions.find((s) => s.id === id)!;
      Object.assign(s, { alive: true, archived: false, state: "idle", stateSince: Date.now() });
      emit("session-updated", s);
      return s;
    },
    close_session: ({ id, archive }) => {
      const i = sessions.findIndex((s) => s.id === id);
      if (archive) {
        Object.assign(sessions[i], { archived: true, alive: false, state: "idle" });
        emit("session-updated", sessions[i]);
      } else {
        sessions.splice(i, 1);
        emit("session-removed", id);
      }
    },
    rename_session: ({ id, title }) => {
      const s = sessions.find((s) => s.id === id)!;
      if (title.trim()) {
        s.titleHistory.push(s.title);
        s.title = title.trim();
        s.titleLocked = true;
      } else s.titleLocked = false;
      emit("session-updated", s);
    },
    pin_identity: ({ id, color, glyph }) => {
      const s = sessions.find((s) => s.id === id)!;
      if (color) Object.assign(s, { color, pinnedColor: true });
      if (glyph) Object.assign(s, { glyph: glyph.toUpperCase().slice(0, 2), pinnedGlyph: true });
      emit("session-updated", s);
    },
    set_visible: () => {},
    save_ui: () => {},
    detect_processes: ({ projectId }) => {
      const p = projects.find((x) => x.id === projectId)!;
      const have = p.processes.map((d) => d.command);
      return [
        { id: "pkg-dev", name: "npm run dev", command: "npm run dev", cwd: ".", env: {}, autoStart: false, autoRestart: false },
        { id: "pkg-start", name: "npm start", command: "npm start", cwd: ".", env: {}, autoStart: false, autoRestart: false },
      ].filter((d) => !have.includes(d.command));
    },
    set_processes_offered: ({ projectId }) => {
      projects.find((x) => x.id === projectId)!.processesOffered = true;
      emit("projects-changed", [...projects]);
    },
    save_process: ({ projectId, def }) => {
      const p = projects.find((x) => x.id === projectId)!;
      const d: ProcessDef = { ...def, id: def.id || crypto.randomUUID().slice(0, 8), name: def.name || def.command };
      const i = p.processes.findIndex((x) => x.id === d.id);
      if (i >= 0) p.processes[i] = d;
      else p.processes.push(d);
      p.processesOffered = true;
      emit("projects-changed", projects.map((x) => ({ ...x, processes: [...x.processes] })));
      return d;
    },
    delete_process: ({ projectId, defId }) => {
      const p = projects.find((x) => x.id === projectId)!;
      p.processes = p.processes.filter((d) => d.id !== defId);
      runs.delete(procKey(projectId, defId));
      emit("projects-changed", projects.map((x) => ({ ...x })));
      emit("process-removed", procKey(projectId, defId));
    },
    sync_configs: () => Object.values(syncCfg),
    sync_status: () => ({ ...syncStatus, anySynced: Object.values(syncCfg).some((c) => c.enabled) }),
    sync_check: ({ projectId }) => ({
      isRepo: true, hasOrigin: !noOrigin.has(projectId), originUrl: noOrigin.has(projectId) ? "" : "https://github.com/dev/demo.git", isGithub: !noOrigin.has(projectId),
      gh: (window as any).__fleetMock?.gh ?? { installed: true, authed: true, user: "dev" },
      mainHasFleetYml: !!syncCfg[projectId]?.config, defaultBranch: "main", enabled: !!syncCfg[projectId]?.enabled, dirtyMain: false,
    }),
    sync_create_repo: ({ projectId, name, private: priv }) => {
      if (!noOrigin.has(projectId)) throw new Error("This project already has an origin remote.");
      noOrigin.delete(projectId);
      return { url: `https://github.com/dev/${name}${priv ? "" : ""}`, branch: "main", skipped: [".env"] };
    },
    sync_config_text: ({ projectId }) => syncCfg[projectId]?.text ?? `project: ${projects.find((p) => p.id === projectId)?.name}\nprocesses: []\nsync:\n  enabled: true\n  branchPrefix: fleet/\n  fetchIntervalSec: 120\n  checks: [npm run lint, npm test]\n  mergeStyle: squash\n  mainIsProtected: false\nclaude:\n  defaultArgs: []\n  titleModel: haiku\n`,
    sync_validate: ({ text }) => mockValidate(text),
    sync_save_config: ({ projectId, text }) => {
      const errs = mockValidate(text);
      if (errs.length) throw new Error(errs.map((e) => `line ${e.line}: ${e.message}`).join("\n"));
      const p = projects.find((x) => x.id === projectId)!;
      const enabled = /enabled:\s*true/.test(text);
      syncCfg[projectId] = {
        projectId, text, errors: [], source: "local", undeployed: true, hasOrigin: true, originUrl: "https://github.com/dev/demo.git",
        defaultBranch: "main", enabled, paused: false, lastDeploy: null, localChanges: localWork[projectId] ?? 0, localAhead: 0,
        config: { project: p.name, processes: [], sync: { enabled, branchPrefix: "fleet/", fetchIntervalSec: 120, checks: [], mergeStyle: "squash", mainIsProtected: false, wipOnQuit: true, chatSync: false }, claude: { defaultArgs: [] } },
      };
      gitOut(p.rootPath, "git update-ref -m 'fleet.yml edit' refs/heads/fleet/config-20261123");
      gitOut(p.rootPath, "git push --porcelain origin refs/heads/fleet/config-20261123:refs/heads/fleet/config-20261123");
      emitSync();
      emit("toast", "fleet.yml saved to fleet/config-20261123 — it reaches main with the next Ship.");
      return syncCfg[projectId];
    },
    sync_set_paused: ({ projectId, paused }) => {
      if (projectId) { const p = projects.find((x) => x.id === projectId)!; p.syncPaused = paused; emit("projects-changed", projects); }
      else syncStatus.pausedAll = paused;
      for (const s of sessions.filter((s) => s.sync && (!projectId || s.projectId === projectId))) {
        s.sync = paused ? { ...s.sync!, badge: "paused", detail: "Sync is paused" } : { ...s.sync!, badge: s.sync!.waiting ? "waiting" : "pushed", detail: null };
        emit("session-updated", { ...s });
      }
      emitSync();
    },
    sync_now: () => {
      if (syncOffline) return;
      for (const s of sessions.filter((s) => s.sync?.badge === "waiting")) {
        gitOut(s.worktreePath ?? "/", `git push --porcelain --atomic origin refs/heads/${s.branch}:refs/heads/${s.branch} refs/heads/fleet-state:refs/heads/fleet-state`, "", 0, true);
        s.sync = { ...s.sync!, badge: "pushed", waiting: 0, lastPushAt: Date.now(), detail: null };
        emit("session-updated", { ...s });
      }
      syncStatus.lastSyncAt = Date.now();
      emitSync();
    },
    sync_session_now: ({ sessionId }) => handlers.sync_now({ sessionId }),
    set_window_appearance: () => {},
    gh_status: () => ({ installed: true, authed: true, user: "dev" }),
    notify_test: () => "Sent.",
    sync_adopt_local: ({ projectId }) => {
      const p = projects.find((x) => x.id === projectId)!;
      let moved = 0;
      for (const s of sessions.filter((s) => s.projectId === projectId && !s.archived && !s.remote && !s.worktreePath)) {
        s.worktreePath = `${p.rootPath}/.fleet/worktrees/${slug(s.title)}`;
        s.branch = "";
        s.sync = { badge: "new", waiting: 0, excluded: [], detail: "Moved into its own synced space" };
        emit("session-updated", { ...s });
        emit("session-respawned", s.id);
        moved++;
      }
      const files = localWork[projectId] ?? 0;
      let newId: string | null = null;
      if (files > 0) {
        const s = handlers.new_session({ projectId, prompt: null, permissionMode: null }) as Session;
        s.title = "Local changes";
        newId = s.id;
        syncSession(s);
      }
      localWork[projectId] = 0;
      syncCfg[projectId] = { ...syncCfg[projectId], localChanges: 0, localAhead: 0 };
      emitSync();
      return { movedSessions: moved, newSession: newId, files };
    },
    sync_others: () => [...others],
    sync_open_session: () => {},
    sync_take_over: ({ sessionId }) => {
      const s = sessions.find((x) => x.id === sessionId)!;
      s.sync = { ...s.sync!, badge: "pushed", detail: "Taken over — this Mac holds it now" };
      emit("session-updated", { ...s });
    },
    sync_continue: ({ projectId, branch, take }) => {
      const o = others.find((x) => x.branch === branch && x.projectId === projectId);
      if (!o) throw new Error("That session's file isn't on GitHub anymore.");
      if (o.activeOn && !take) throw new Error(`ACTIVE:Active on ${o.activeOn} · ${o.activeAgo}`);
      const s = handlers.new_session({ projectId, prompt: `I'm continuing this session on another computer (it was last on ${o.lastMachine}).`, permissionMode: null }) as Session;
      Object.assign(s, { title: o.title, glyph: o.glyph, color: o.color, branch, recap: o.recap, pinnedGlyph: true, pinnedColor: true, sync: { badge: "pushed", waiting: 0, excluded: [], detail: `Continued from ${o.lastMachine}` } });
      emit("session-updated", { ...s });
      others = others.filter((x) => x !== o);
      emit("sync-others", [...others]);
      return s;
    },
    sync_fix_conflict: ({ projectId, branch, why }) => {
      const text = `${why ?? "This branch conflicts with main."}\n\nPlease fix the conflict on this branch (${branch}): git merge origin/main …`;
      const own = sessions.filter((x) => x.projectId === projectId && x.branch === branch).sort((a, b) => Number(!a.alive) - Number(!b.alive))[0];
      if (own?.alive && own.state === "working") throw new Error(`${own.glyph} · ${own.title} is working right now. Let it finish, then try again.`);
      const fixing = { waiting: 0, excluded: [], badge: "conflict" as const, detail: "Claude is fixing the conflict" };
      if (own) {
        if (own.alive) sent.push({ sessionId: own.id, text });
        Object.assign(own, { alive: true, archived: false, state: "working", stateSince: Date.now(), pendingQuestion: null, lastPrompt: text, sync: { ...own.sync, ...fixing } });
        emit("session-updated", { ...own });
        return own;
      }
      const o = others.find((x) => x.projectId === projectId && x.branch === branch);
      const s = handlers.new_session({ projectId, prompt: text, permissionMode: null }) as Session;
      Object.assign(s, { title: o?.title ?? `Fix conflict: ${branch}`, glyph: o?.glyph ?? s.glyph, color: o?.color ?? s.color, branch, sync: fixing });
      emit("session-updated", { ...s });
      others = others.filter((x) => x !== o);
      emit("sync-others", [...others]);
      return s;
    },
    gh_repos: () => [{ nameWithOwner: "dev/atrium-v2", description: "ERP", isPrivate: true, updatedAt: "2026-11-23T10:00:00Z" }, { nameWithOwner: "dev/site", description: null, isPrivate: false, updatedAt: "2026-11-01T10:00:00Z" }],
    sync_clone: ({ repo }) => handlers.add_project({ path: `/Users/dev/code/${repo.split("/")[1]}` }),
    ship_info: ({ projectId }) => {
      const local = sessions.filter((s) => s.projectId === projectId && !s.archived && s.branch && s.sync);
      const candidates = [
        ...local.map((s) => ({ branch: s.branch, title: s.title, glyph: s.glyph, color: s.color, commits: 2, stat: "3 files changed, 40 insertions(+), 2 deletions(-)", machine: "MacBook Pro", state: s.state, ready: s.state !== "working" && s.sync?.badge === "pushed", reason: s.state === "working" ? "Working" : s.sync?.badge === "conflict" ? "Has a conflict" : s.sync?.badge !== "pushed" ? "Not pushed yet" : null, sessionId: s.id })),
        ...others.filter((o) => o.projectId === projectId).map((o) => ({ branch: o.branch, title: o.title, glyph: o.glyph, color: o.color, commits: o.commits, stat: "1 file changed", machine: o.lastMachine, state: o.state, ready: true, reason: null, sessionId: null })),
      ];
      const cfg = syncCfg[projectId]?.config?.sync ?? {};
      return { candidates, checks: ["npm run lint", "npm test"], deployCommand: cfg.deployCommand ?? "./deploy.sh all", mergeStyle: "squash", protected: false, defaultBranch: "main", deploys: [...deploys], shipping: false };
    },
    ship_run: ({ projectId, branches }) => {
      const log = (text: string) => emit("ship-log", { projectId, text });
      const results: [string, any][] = [];
      for (const b of branches) {
        emit("ship-progress", { projectId, step: "merge", branch: b });
        log(`\n$ npm run lint\n✓ lint\n$ npm test\n`);
        if (shipConflicts.has(b)) {
          results.push([b, { kind: "conflict", files: ["src/fee.ts"], output: "CONFLICT (content): Merge conflict in src/fee.ts" }]);
          const s = sessions.find((x) => x.branch === b);
          if (s) { Object.assign(s, { state: "needs_reply", pendingQuestion: "Ship skipped this branch: it conflicts with main in src/fee.ts.", sync: { ...s.sync!, badge: "conflict", detail: "Conflicts with main in src/fee.ts" } }); emit("session-updated", { ...s }); }
        } else {
          results.push([b, { kind: "merged", commit: Math.random().toString(16).slice(2, 10) }]);
          const s = sessions.find((x) => x.branch === b);
          if (s) { Object.assign(s, { branch: "", sync: { badge: "new", waiting: 0, excluded: [], detail: "Shipped ✓ — the next prompt starts a new branch" } }); emit("session-updated", { ...s }); }
          others = others.filter((o) => o.branch !== b);
        }
      }
      emit("sync-others", [...others]);
      log("$ ./deploy.sh all\ndeployed\n[exit 0]\n");
      const tag = `deploy-20261123-14${String(10 + deploys.length).padStart(2, "0")}`;
      const merged = results.filter(([, r]) => r.kind === "merged").map(([b]) => b);
      deploys.push({ tag, commit: "abc123", branches: merged, machine: "MacBook Pro", at: new Date().toISOString(), result: "deployed" });
      syncCfg[projectId] = { ...syncCfg[projectId], lastDeploy: deploys[deploys.length - 1] };
      emitSync();
      const outcome = { results, mainCommit: "abc123", pushed: true, tag, deployCode: 0, deployOutput: "deployed", error: null, others: [] };
      setTimeout(() => emit("ship-done", { projectId, outcome }), 0);
      return outcome;
    },
    ship_redeploy: ({ projectId }) => { emit("ship-log", { projectId, text: "$ ./deploy.sh all\n[exit 0]\n" }); const prev = deploys[deploys.length - 2]; if (!prev) throw new Error("There's no previous deploy to go back to."); deploys.push({ ...prev, tag: `${prev.tag} (redeployed)`, result: "redeployed", at: new Date().toISOString() }); return prev.tag; },
    ship_history: () => deploys.flatMap((d) => d.branches.map((b: string) => ({ commit: `c-${b}`, subject: `Feature ${b}`, at: Math.floor(Date.now() / 1000), branch: b }))),
    ship_undo: ({ projectId, commit }) => { const b = `fleet/revert-${commit}`; others.push({ projectId, branch: b, glyph: "↶", color: "#8B949E", title: `Undo: ${commit}`, recap: [], state: "done", prompts: 0, lastMachine: "MacBook Pro", updatedAt: new Date().toISOString(), commits: 1, needsRebase: false }); emit("sync-others", [...others]); return b; },
    sync_workflows: () => (window as any).__fleetMock?.workflows ?? [],
    sync_limit_workflows: () => { (window as any).__fleetMock.workflows = []; return "fleet/config-20261123"; },
    gh_terminal: ({ action }) => `rterm:gh:${++termN}${action === "install" ? "" : ""}`,
    process_start: ({ projectId, defId }) => {
      const key = procKey(projectId, defId);
      const r: ProcessRun = { key, projectId, defId, pid: 4242, status: "running", detectedPorts: [], startedAt: Date.now(), restarts: 0, stopping: false, detached: false };
      runs.set(key, r);
      emitRun(r);
      write(key, `$ ${findDef(projectId, defId)?.command}\r\n  VITE ready\r\n  ➜  Local: http://localhost:5173/\r\n`);
      setTimeout(() => {
        if (runs.get(key)?.status === "running") emitRun(Object.assign(runs.get(key)!, { detectedPorts: [5173] }));
      }, 150);
      return r;
    },
    process_stop: ({ projectId, defId }) => {
      const r = runs.get(procKey(projectId, defId));
      if (r) emitRun(Object.assign(r, { status: "stopped", detectedPorts: [], pid: null }));
    },
    process_restart: ({ projectId, defId }) => handlers.process_start({ projectId, defId }),
    pty_clear: () => {},
    send_to_session: ({ sessionId, text }) => {
      sent.push({ sessionId, text });
    },
    quit_app: () => {},
    git_watch: () => {},
    git_status: () => ({ isRepo: true, status: statusOf(), top: projects[0]?.rootPath ?? "/", merging: false }),
    git_diff: ({ path, staged }) => {
      const f = repo.files[path];
      const original = (staged ? f.head : f.index ?? (f.head == null ? "" : f.head)) ?? "";
      const modified = (staged ? f.index : f.work) ?? "";
      const o = original.split("\n").length - 1, m = modified.split("\n").length - 1;
      return { original, modified, binary: false, diff: { header: "diff\n", hunks: original === modified ? [] : [{ header: `@@ -1,${o} +1,${m} @@`, oldStart: 1, oldLen: o, newStart: 1, newLen: m, lines: [] }] } };
    },
    git_hunk: ({ root, path, action }) => {
      const f = repo.files[path];
      if (action === "stage") f.index = f.work;
      if (action === "unstage") f.index = f.head;
      if (action === "discard") f.work = f.index;
      gitOut(root, `git apply ${action === "stage" ? "--cached" : action === "unstage" ? "--cached --reverse" : "--reverse"} --recount -`);
    },
    git_stage: ({ root, paths }) => { for (const p of paths) repo.files[p].index = repo.files[p].work; gitOut(root, `git add -A -- ${paths.join(" ")}`); },
    git_unstage: ({ root, paths }) => { for (const p of paths) repo.files[p].index = repo.files[p].head; gitOut(root, `git restore --staged -- ${paths.join(" ")}`); },
    git_discard: ({ root, paths }) => { for (const p of paths) repo.files[p].work = repo.files[p].index; gitOut(root, `git restore -- ${paths.join(" ")}`); },
    git_commit: ({ root, message, push }) => {
      const staged = Object.values(repo.files).some((f) => f.index !== f.head);
      if (!staged) throw gitErr("Nothing to commit — stage some changes first.");
      for (const f of Object.values(repo.files)) f.head = f.index;
      repo.log.unshift({ hash: Math.random().toString(16).slice(2, 14), parents: [repo.log[0].hash], author: "Dev", time: Math.floor(Date.now() / 1000), subject: message.split("\n")[0], refs: "HEAD -> main" });
      repo.ahead += 1;
      gitOut(root, `git commit -m '${message}'`, `[main ${repo.log[0].hash.slice(0, 7)}] ${message}`);
      if (push) handlers.git_remote({ root, op: "push" });
    },
    git_remote: ({ root, op }) => {
      if (op === "push" && (window as any).__fleetMock.rejectPush) {
        gitOut(root, "git push", " ! [rejected]        main -> main (fetch first)", 1);
        throw gitErr("Push was rejected: the remote has commits you don't have. Pull first, then push.", "pull", "Pull");
      }
      if (op === "push" || op === "set_upstream_push") repo.ahead = 0;
      gitOut(root, `git ${op === "set_upstream_push" ? "push -u origin main" : op}`, op === "push" ? "To origin\n   abc..def  main -> main" : "");
    },
    git_cancel: () => {},
    git_run: ({ root, args }) => { if (args[0] === "stash") repo.stashes.unshift({ name: "stash@{0}", message: "On main: wip", time: Math.floor(Date.now() / 1000) }); gitOut(root, `git ${args.join(" ")}`); return ""; },
    git_log: () => repo.log,
    git_commit_files: () => [{ status: "M", path: "src/fee.ts" }],
    git_commit_diff: () => ({ original: "a\n", modified: "b\n", binary: false, diff: { header: "", hunks: [] } }),
    git_branches: () => ({ branches: [{ name: "main", full: "refs/heads/main", remote: false, upstream: repo.upstream, ahead: repo.ahead, behind: 0, gone: false, time: Math.floor(Date.now() / 1000), head: true, worktree: null }, { name: "feature/x", full: "refs/heads/feature/x", remote: false, upstream: null, ahead: 0, behind: 0, gone: false, time: 1700000000, head: false, worktree: null }], worktrees: [{ path: projects[0]?.rootPath ?? "/", head: "a1", branch: "main", bare: false, detached: false }] }),
    git_stashes: () => repo.stashes,
    git_write_message: () => "Use the Pix fee label in checkout",
    fs_list: ({ dir }) => {
      const root = projects.find((p) => dir.startsWith(p.rootPath))?.rootPath ?? dir;
      const rel = dir === root ? "" : dir.slice(root.length + 1) + "/";
      const names = new Map<string, boolean>();
      for (const [p, f] of Object.entries(repo.files)) {
        if (f.work == null || !p.startsWith(rel)) continue;
        const rest = p.slice(rel.length);
        const [first, ...more] = rest.split("/");
        names.set(first, more.length > 0);
      }
      return [...names].sort((a, b) => Number(b[1]) - Number(a[1]) || a[0].localeCompare(b[0])).map(([name, isDir]) => ({ name, path: `${dir}/${name}`, isDir, ignored: false }));
    },
    fs_read: ({ path }) => ({ text: repo.files[relOf(path)]?.work ?? "", mtime: 1, binary: false, tooLarge: false }),
    fs_write: ({ path, text }) => { const r = relOf(path); repo.files[r] ??= { head: null, index: null, work: null }; repo.files[r].work = text; emit("git-changed", projects[0]?.rootPath); return 2; },
    fs_create: ({ path }) => { repo.files[relOf(path)] = { head: null, index: null, work: "" }; },
    fs_rename: () => {}, fs_trash: () => {}, fs_reveal: () => {},
    quick_open_files: () => Object.keys(repo.files).filter((p) => repo.files[p].work != null),
    search_files: ({ query }) => Object.entries(repo.files).filter(([, f]) => f.work?.includes(query)).map(([path, f]) => ({ path, matches: f.work!.split("\n").flatMap((l, i) => (l.includes(query) ? [{ line: i + 1, col: l.indexOf(query) + 1, len: query.length, text: l }] : [])) })),
    replace_all: ({ paths, query, replacement }) => { let n = 0; for (const p of paths) { const f = repo.files[p]; if (!f.work) continue; n += f.work.split(query).length - 1; f.work = f.work.split(query).join(replacement); } return n; },
    format_text: () => null,
    head_text: ({ path }) => repo.files[relOf(path)]?.head ?? null,
    quick_edit: ({ text, start, end }) => text.split("\n").slice(start - 1, end).map((l: string) => l.toUpperCase()).join("\n"),
    lsp_start: () => { throw new Error("no LSP in the browser"); },
    lsp_send: () => {}, lsp_stop: () => {},
    get_settings: () => null, save_settings: () => {}, reveal_settings: () => {},
    worktree_finish: () => {},
    // ---- ssh + consoles
    ssh_save: ({ projectId, conn }) => {
      const p = projects.find((x) => x.id === projectId)!;
      const c = { ...conn, id: conn.id || crypto.randomUUID(), name: conn.name || conn.host };
      const i = p.sshConnections.findIndex((x) => x.id === c.id);
      if (i >= 0) p.sshConnections[i] = c; else p.sshConnections.push(c);
      emit("projects-changed", projects.map((x) => ({ ...x, sshConnections: [...x.sshConnections] })));
      return c;
    },
    ssh_delete: ({ projectId, connId }) => {
      const p = projects.find((x) => x.id === projectId)!;
      p.sshConnections = p.sshConnections.filter((c) => c.id !== connId);
      emit("projects-changed", projects.map((x) => ({ ...x })));
    },
    ssh_connect: ({ connId }) => {
      const conn = projects.flatMap((p) => p.sshConnections).find((c) => c.id === connId)!;
      const server = conn.host.toLowerCase();
      sshWanted.add(connId);
      const shared = () => projects.flatMap((p) => p.sshConnections).filter((c) => c.id !== connId && c.host.toLowerCase() === server && sshWanted.has(c.id)).length;
      if (sshServers.has(server)) { emit("ssh-status", { connId, state: "connected", since: Date.now(), sharedWith: shared() }); return; }
      emit("ssh-status", { connId, state: "connecting", since: Date.now(), sharedWith: 0 });
      setTimeout(() => { sshServers.add(server); sshConnects.push(server); emit("ssh-status", { connId, state: "connected", since: Date.now(), sharedWith: shared() }); }, 150);
    },
    ssh_disconnect: ({ connId }) => { sshWanted.delete(connId); emit("ssh-status", { connId, state: "disconnected", since: Date.now(), sharedWith: 0 }); },
    ssh_statuses: () => [],
    ssh_master_pty: ({ connId }) => `ssh:${connId}`,
    console_open: ({ sessionId }) => { const id = `console:${sessionId}:${++termN}`; return id; },
    terminal_close: () => {},
    remote_terminal_open: ({ connId }) => `rterm:${connId}:${++termN}`,
  };

  (window as any).__fleetMock = {
    setState(index: number, state: SessionState, question?: string) {
      const s = sessions[index];
      Object.assign(s, { state, stateSince: Date.now(), pendingQuestion: question ?? null });
      emit("session-updated", { ...s });
    },
    setMetrics(index: number, m: Partial<SessionMetrics>) {
      const s = sessions[index];
      const zero = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0, unpriced: 0 };
      s.metrics = { main: zero, sub: zero, turns: 0, turnMs: 0, waitMs: 0, turnWait: 0, ...s.metrics, ...m };
      emit("session-updated", { ...s });
    },
    retitle(index: number, title: string) {
      const s = sessions[index];
      s.titleHistory.push(s.title);
      s.title = title;
      emit("session-updated", { ...s });
    },
    sessions,
    sent,
    ptyWrites,
    repo,
    remoteBranches,
    /** A session another Mac pushed to GitHub (shown under "On other computers"). */
    otherSession(projectIndex: number, o: Record<string, unknown>) {
      others.push({ projectId: projects[projectIndex].id, glyph: "WM", color: "#7EE787", title: "Watermark missing on PDFs", recap: ["Watermark now drawn on every page"], state: "done", prompts: 3, lastMachine: "iMac", updatedAt: new Date(Date.now() - 120000).toISOString(), commits: 2, needsRebase: false, branch: "fleet/wm-watermark-missing", activeOn: null, activeAgo: null, ...o });
      emit("sync-others", [...others]);
    },
    /** Uncommitted changes in a synced project's main folder. */
    localWork(projectIndex: number, n: number) {
      const p = projects[projectIndex];
      localWork[p.id] = n;
      syncCfg[p.id] = { ...syncCfg[p.id], localChanges: n };
      emitSync();
    },
    takenOver(index: number, machine: string) {
      const s = sessions[index];
      s.sync = { ...(s.sync ?? { waiting: 0, excluded: [] }), badge: "taken_over", detail: `Active on ${machine} · 2m ago — not pushed. Take over to continue here.` } as Session["sync"];
      emit("session-updated", { ...s });
    },
    shipConflicts,
    /** Project at `index` has no origin remote yet. */
    noOriginAt(index: number) { noOrigin.add(projects[index].id); },
    workflows: [] as unknown[],
    /** A synced session's prompt finished: commit + state + push (or queue when offline). */
    syncStop(index: number) {
      const s = sessions[index];
      Object.assign(s, { state: "done", stateSince: Date.now() });
      syncSession(s);
    },
    setOffline(off: boolean) {
      syncOffline = off;
      if (!off) handlers.sync_now({});
      else emitSync();
    },
    /** Put a broken fleet.yml on "main" for a synced project. */
    brokenConfig(projectIndex: number, line: number, message: string) {
      const p = projects[projectIndex];
      syncCfg[p.id] = { ...syncCfg[p.id], errors: [{ line, message }], source: "main", undeployed: false };
      emitSync();
    },
    excluded(index: number, files: string[]) {
      const s = sessions[index];
      s.sync = { ...(s.sync ?? { badge: "pushed", waiting: 0 }), excluded: files } as Session["sync"];
      emit("session-updated", { ...s });
      emit("sync-excluded", { sessionId: s.id, files });
    },
    rejectPush: false,
    sshConnects,
    touch(index: number, rel: string) {
      const s = sessions[index];
      const abs = `${projects[0].rootPath}/${rel}`;
      if (!s.filesTouched.includes(abs)) s.filesTouched.push(abs);
      emit("session-updated", { ...s, filesTouched: [...s.filesTouched] });
    },
    crash(projectIndex: number, defIndex: number, code = 1) {
      const p = projects[projectIndex];
      const key = procKey(p.id, p.processes[defIndex].id);
      write(key, "Error: Cannot find module 'left-pad'\r\n");
      emitRun(Object.assign(runs.get(key)!, { status: "crashed", exitCode: code, detectedPorts: [], pid: null }));
    },
  };

  return {
    isTauri: false,
    invoke: async <T,>(cmd: string, args?: Record<string, unknown>) => {
      const h = handlers[cmd];
      if (!h) throw new Error(`mock: unknown command ${cmd}`);
      return h(args ?? {}) as T;
    },
    listen: async (event, cb) => {
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event)!.add(cb as Listener);
      return () => listeners.get(event)!.delete(cb as Listener);
    },
    channel: () => {
      const c = { onmessage: (_: ArrayBuffer | Uint8Array | string) => {}, raw: null as unknown };
      c.raw = c;
      return c;
    },
    pickFolder: async () => `/Users/dev/code/project-${++folderN}`,
    openUrl: async (url) => void window.open(url, "_blank"),
    setTitle: async (t) => void (document.title = t),
  };
}

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

fleet is a macOS desktop app (Tauri 2: Rust backend + React/TypeScript frontend) for running many Claude Code sessions at once. Every session is the real `claude` CLI in a real PTY; fleet adds identity, live state, navigation, a git panel, dev-process management, an editor, consoles and SSH connections around it. `SPEC.md` is the original product spec; `README.md` documents behaviour per feature.

## Commands

Rust is installed via rustup; in a fresh shell run `source ~/.cargo/env` first (npm scripts call `cargo`).

```sh
npm install
npm run tauri dev          # dev app; runs build:hook first, then Vite on :1420, then the Rust app
npm run dev                # UI only, in a browser, against the in-memory mock backend
npm run build:hook         # builds the fleet-hook + fleet-remote-mcp sidecars into src-tauri/binaries/
npm run typecheck          # tsc --noEmit
./scripts/release.sh       # .app + .dmg (ad-hoc signed; Developer ID + notarization via APPLE_* env vars)

npm run test:rust          # cd src-tauri && cargo test
cd src-tauri && cargo test git::tests::partial_line_patch    # one Rust test (name filter)
cd fleet-remote-mcp && cargo test                            # the remote-tools crate has its own tests
npm run test:e2e           # Playwright (WebKit) against `npm run dev` + mock backend
npx playwright test -g "⌘P opens a file"                     # one e2e test by title
npx playwright test tests/e2e/git-editor.spec.ts --workers=1 # one file; timing tests are flaky under parallel load
```

`tauri.conf.json` lists `binaries/fleet-hook` and `binaries/fleet-remote-mcp` as `externalBin`, so the Tauri build fails unless `npm run build:hook` has produced them (the `before*Command`s do this).

## Architecture

### Three Rust crates
- `src-tauri/` — the app (lib `fleet_lib`). Everything is in `src/lib.rs` (state `Fleet`, most `#[tauri::command]`s, setup/exit) plus modules per area.
- `fleet-hook/` — tiny binary registered as Claude Code command hook for every event. Reads hook JSON on stdin, tags it with `FLEET_SESSION_ID`, writes one line to the Unix socket `FLEET_SOCKET`, always exits 0. Standalone crate (no Tauri build script) on purpose.
- `fleet-remote-mcp/` — stdio MCP server giving claude `bash/read/write/edit/glob/grep/ls` that run on a server through fleet's SSH ControlMaster (`--local` mode runs the same scripts locally; used by its tests).

### Session state pipeline
`spawn_claude` (lib.rs) launches `claude --session-id|--resume <uuid> --settings <app-support>/fleet-hooks.json …` in a PTY with the user's login-shell env (`shell_env.rs`, captured once with `$SHELL -ilc env`, stripping inherited `CLAUDE_CODE_*` markers). Hooks → `fleet-hook` → `socket.rs` → `hooks::parse` → `state_machine::apply` (pure, unit-tested) → `session-updated` event. Stop triggers `titler.rs` (one-off `claude -p` with hooks disabled) for live titles and `transcript.rs` for the context meter. Hooks are only ever injected via `--settings`; fleet never edits `~/.claude`.

### PTYs and terminals
`pty.rs` `PtyManager` owns every child process: sessions (id = session uuid), processes (`proc:<project>:<def>`), consoles (`console:<session>:<n>`), remote shells (`rterm:<conn>:<n>`) and SSH masters (`ssh:<serverId>`). Output goes to a replay ring and is flushed to a Tauri `Channel` every 16 ms. `spawn(id, …, on_exit(code, current))`: `current` is false if that PTY was killed or replaced under the same id — exit callbacks must check it. Exited PTYs stay registered so their scrollback can replay.

Frontend `src/terminal/termPool.ts` keeps one xterm per id alive offscreen and moves its DOM node into whichever view shows it (fast switching); only the 8 most-recently-shown keep the WebGL renderer.

### Locking rule
`Fleet.store` is a `parking_lot::Mutex` (not re-entrant). Several callers (`new_session`, `restart_session`, `restore_sessions`) hold it while calling `spawn_claude`, so nothing reachable from there may lock the store again (e.g. use the `project` argument, not `ssh::find_conn`). Re-locking deadlocks and freezes the app.

### Frontend
- `src/ipc.ts` picks the real Tauri backend or `src/mockBackend.ts` (in-memory, used by `npm run dev` and all Playwright tests; exposes `window.__fleetMock` helpers). **Every new Tauri command needs a mock handler** or the e2e suite breaks.
- Zustand stores: `store/store.ts` (projects, sessions, runs, UI, SSH status, consoles), `store/git.ts`, `store/editor.ts` (tabs: file/diff/commit/conflict), `store/settings.ts` (settings.json schema; backend reads a few keys via `settings.rs`). Selectors must return stable values — a selector that builds a new object each call loops forever under zustand 5.
- `src/commands.ts` is the single registry of actions + default shortcuts, used by the global keydown handler, the ⌘K palette and the Settings shortcut editor. Shortcuts are ⌘-based; a combo is only taken if a command is bound to it (and its `run` doesn't return `false`), so other keys reach xterm/Monaco.
- Editor: Monaco bundled locally (`src/editor/monaco.ts`; worker imports use `monaco-editor/<path>` because of the package's exports map). LSP via Monaco's built-in `monaco.lsp.MonacoLspClient` over a custom transport (`src/editor/lsp.ts`) to servers spawned by `src-tauri/src/lsp.rs`, which patches `rootUri` into `initialize`. typescript-language-server + typescript are bundled as resources.

### Themes
- `src/styles.css` uses only tokens (`--panel`, `--accent`, `--state-*`, `--radius*`, `--font-ui`…). Never hard-code a color in CSS or TSX; use a token (`var(--state-done)` etc.). SVG colors go through `style={{ fill/stroke }}` because presentation attributes don't resolve `var()`.
- `src/themes/defs.ts` defines every theme's tokens, terminal (xterm) colors, editor syntax and window treatment; `src/themes/effects.css` holds per-theme decoration scoped by `:root[data-theme=…]` (motion gated by `data-effects` and reduced-motion). `applyTheme()` (`src/themes/index.ts`) sets tokens on `:root`, loads bundled fonts, re-themes all xterms (`termPool.applyTheme`) and Monaco (`setEditorTheme`), and calls `set_window_appearance` (Rust: vibrancy for Glass, light/dark chrome). The window is `transparent` with `macOSPrivateApi`; opaque themes paint `--app-bg`.
- Adding a theme = one entry in `defs.ts` (+ optional rules in `effects.css`); `tests/e2e/themes.spec.ts` checks its contrast.

### Git, processes, SSH
- `git.rs` holds pure parsers (porcelain v2, numstat, log, branches, worktrees, stash), hunk/line patch building (`git apply --cached --recount`) and stderr→readable-error mapping; `git_cmds.rs` runs the git CLI (recorded actions emit `git-output` for the Output drawer) and the debounced file watcher (`git-changed`, `fs-changed`).
- Project processes: definitions in `<project>/.fleet/project.json` (`processes.rs`), runtime in `proc_runtime.rs` (stop = SIGINT → TERM after 5 s → KILL after 10 s on the process group; ports via `ps` + `lsof` every 5 s).
- SSH (`ssh.rs`): connections are per project but share one ControlMaster per server (`server_id` hashes host/port/identity/options); `wanted` refcounts which project connections hold it open. Remote claude sessions run locally with `--mcp-config` (fleet-remote-mcp), `--disallowedTools` for local file/shell tools, and `--append-system-prompt` (keep it last before the positional prompt — the preceding flags are variadic).

### GitHub sync (`src-tauri/src/sync/`)
- Tauri-free engine, tested against a local bare repo (`gitx::testutil::Fixture`): `gitx.rs` (logged git runner + plumbing `commit_files`/`update_ref`), `fleetyml.rs` (YAML → JSON Schema in `schema/fleet.schema.json`, line numbers via marked-yaml spans), `state_branch.rs` (orphan `fleet-state` branch, per-file merge by `updatedAt`, `replay` after a rejected push), `autocommit.rs` (what runs on Stop), `pusher.rs`.
- **Every push goes through `pusher.rs`.** main/master/default-branch needs a `ship::MainPush`, constructible only inside `ship.rs`; the test `nothing_else_pushes` fails if a `"push"` git invocation appears in production code elsewhere (the git panel's pushes in `git_cmds.rs` must call `pusher::guard_manual`).
- `mod.rs` is the Tauri glue: config cache per project, `on_stop` (waits for the titler via `SyncState.titling`), the offline queue (`sync-queue.json`), the fetch loop, events `sync-configs`/`sync-status`/`sync-excluded`. Synced sessions always get a worktree (`worktree::create_detached`); the branch is named on the first Stop.
- Stage B/C: `presence.rs` (refs/fleet/presence/<branch> tag objects; `holder()`; every session push carries a lease on it), `load.rs` (load sequence after each fetch, "On other computers", Continue here, take over, Add project from GitHub), `adopt.rs` (moves main-folder work + pre-sync sessions into worktrees; copies claude transcripts to the new cwd's `~/.claude/projects/<encoded>` dir so `--resume` works), `ship.rs` (engine; owns `MainPush`) + `ship_cmds.rs` (Tauri side, streaming `ship-log`, redeploy, undo, Actions cost guard), `redact.rs` (opt-in chat sync).
- `FLEET_APP_DIR` overrides the app-support folder (simulate a second Mac); long folders get a short socket in `$TMPDIR`.

### Persistence
`~/Library/Application Support/fleet/`: `state.json` (projects, sessions, detached processes; debounced 2 s + on exit), `ui.json`, `settings.json`, `fleet-hooks.json`, `fleet.sock`, `ssh/*.sock`, `remote/*.mcp.json`, `fleet-config/` (unshipped fleet.yml edits + last valid config), `sync-queue.json`, `metrics.jsonl` (append-only usage/turn log behind the Usage sheet; `usage.rs`).

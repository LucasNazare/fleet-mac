# fleet

A macOS terminal + light IDE for running many Claude Code sessions in parallel.
Every session is the real `claude` CLI in a real PTY; fleet adds identity, live
state, navigation and (in later milestones) git, dev processes and an editor.

Spec: [SPEC.md](SPEC.md). Status: **all six milestones are built** — terminals, state and identity, project processes, git panel, editor, polish.

## Install / run

Requirements: macOS 14+, Rust (`rustup`), Node 20+, `claude` on your login shell's PATH.

```sh
npm install
npm run tauri dev        # dev app (builds fleet-hook first)
./scripts/release.sh     # .app + .dmg in src-tauri/target/release/bundle
```

`release.sh` ad-hoc signs by default (runs on this Mac). Set `APPLE_SIGNING_IDENTITY`
(and `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID`, or `APPLE_API_KEY`/`APPLE_API_ISSUER`/
`APPLE_API_KEY_PATH`) to sign with a Developer ID and notarize.

Tests:

```sh
npm run test:rust        # hook parser, state machine, identity, transcript, PTY batching
npm run test:e2e         # Playwright against the UI with an in-memory mock backend
```

`npm run dev` alone opens the UI in a browser with the mock backend
(`src/mockBackend.ts`); `window.__fleetMock.setState(i, "needs_permission", "run x?")`
drives state changes by hand.

## How it works

```
React UI (xterm.js pool, zustand)  ⇄  Tauri commands/events  ⇄  Rust backend
                                                                 ├─ PtyManager: one PTY per claude, 16 ms batched output, replay ring
                                                                 ├─ socket.rs: ~/Library/Application Support/fleet/fleet.sock
                                                                 ├─ state_machine.rs: hook events → session state
                                                                 └─ titler.rs: claude -p --model haiku → title + recap
claude (in PTY) ── hook ──> fleet-hook ── one JSON line ──> fleet.sock
```

- Sessions start as `claude --session-id <uuid> --settings <fleet-hooks.json> [prompt]`
  in the project folder, with your login-shell environment (`$SHELL -ilc env`), plus
  `FLEET_SESSION_ID`, `FLEET_SOCKET` and `TERM=xterm-256color`.
- Each terminal is kept alive in the frontend; switching only moves its DOM node
  (~40 ms measured in the e2e test). The 8 most recently shown keep their WebGL renderer.
- ⌘-shortcuts belong to fleet; every Ctrl combination goes straight to the terminal.

### Hooks

fleet never edits your `~/.claude/settings.json`. It writes
`~/Library/Application Support/fleet/fleet-hooks.json` and passes it with `--settings`
**only to the sessions it starts**, so your own hooks keep running alongside.

Each registered event runs `fleet-hook` (a ~370 KB Rust binary, `fleet-hook/`) which reads
the hook JSON from stdin, tags it with `FLEET_SESSION_ID`, writes one line to the socket
and exits 0 within ~1 s no matter what — it cannot block or change Claude Code.

| Hook event | fleet state | Notes |
| --- | --- | --- |
| SessionStart | idle (kept on `compact`) | stores `transcript_path`, updates the Claude session id (it changes on `/clear`) |
| UserPromptSubmit | working | first prompt becomes the first title |
| PreToolUse / PostToolUse | working | Edit/Write/MultiEdit/NotebookEdit paths → `filesTouched`; refreshes context meter |
| PermissionRequest | needs_permission | question built from the tool, e.g. "run ./deploy.sh all?" |
| Notification `permission_prompt` | needs_permission | fallback; doesn't overwrite PermissionRequest's question |
| Notification `elicitation_dialog`, `agent_needs_input` | needs_reply | |
| Notification `idle_prompt` | idle, if we still thought it was working | covers Esc-interrupts, which fire no Stop |
| Stop | done, or needs_reply if `last_assistant_message` ends with "?" | regenerates title + recap |
| StopFailure | failed | API error ended the turn |
| SessionEnd | idle | a non-zero PTY exit marks it failed |

Differences from SPEC.md, following the current hooks docs: `PermissionRequest` is used
as the dedicated permission event (it fires immediately; the `permission_prompt`
notification arrives ~6 s later). Stop uses the `last_assistant_message` field instead of
reading the transcript. `idle_prompt` is not mapped to needs_reply (it fires 60 s after
every reply, which would hide "done"); it only un-sticks sessions that were interrupted.

### Usage: cost and turn time

- **Session header**: estimated cost so far (`$1.23 · 25% agents` = the share spent by sub-agents) and turn time (`◷ 2m 05s`, live while Claude works, else the last turn). Hover for the token breakdown, the average turn and how much of it was spent waiting on you (permission prompts, questions).
- **Status bar**: today's estimated spend across every project; click it, or run "Show usage" from ⌘K, for the Usage sheet: cost, sub-agent share, tokens, turns, average and longest turn and time waiting on you, per project and per day (today / 7 / 30 days).
- Tokens come from the claude transcript (`~/.claude/projects/…/<id>.jsonl`) and its sub-agent transcripts (`<id>/subagents/*.jsonl`). They're read incrementally on every tool use and Stop, deduplicated by request id, and priced in `src-tauri/src/usage.rs` at Anthropic API list prices (5-minute/1-hour cache writes, per-model cache-read rates, fast mode ×2). On a subscription this estimates relative spend rather than a bill. Tokens from unknown models are counted but not priced.
- A turn is `UserPromptSubmit` → `Stop` / `StopFailure` / Esc. A session killed mid-turn records no turn.
- Rollups come from `metrics.jsonl` in the app-support folder: one line per day of new usage per session (dated by the transcript line, so the first scan of an older session lands on the right days) and one per finished turn.

### Project processes

- Defined per project in `<root>/.fleet/project.json` (`{"processes": [...]}`) so a team can
  commit it; "Keep .fleet/project.json out of git" adds it to `.git/info/exclude` instead.
- On first open fleet offers `dev`/`start`/`watch`/`serve` from package.json (using the
  pnpm/yarn/bun/npm runner its lockfile implies), Procfile entries, `docker compose up` and
  Makefile `dev`/`run`/`serve`/`watch`/`start`/`up` targets. Adding one never starts it.
- Each process runs as `$SHELL -c "trap '' HUP; <command>"` in its own PTY and process group,
  with your login-shell environment plus the process's env vars.
- Ports: every 5 s, `ps` builds the process tree and `lsof -iTCP -sTCP:LISTEN` finds what the
  tree listens on; ports become links to `http://localhost:<port>`.
- Stop: SIGINT to the process group, SIGTERM after 5 s, SIGKILL after 10 s.
  Crash: red row + notification; with auto-restart, retries after 1, 2, 4 … 30 s.
- Quit with processes running asks: **Stop all** (default) or **Leave running**. Left-running
  processes are re-adopted on the next launch (status and ports, no output) and can be stopped.
  Because processes ignore SIGHUP (that's what lets them survive), a force-killed fleet also
  leaves them running; the next launch only re-adopts ones you chose to leave running.
- Select text in any terminal and press ⌘⇧S ("Send to session…") to paste it into a session's
  prompt with a "From npm run dev in atrium-api:" header — bracketed paste, not submitted.
- Keys: ↵ on a process row opens it, ⌘R restarts, ⌘. stops, ⌘F searches its output.

### Git panel

⌘G shows or hides it (⌘⌥B too).

- Runs the `git` CLI in the selected session's checkout (its worktree if it has one).
  Every action — and its output — is listed in the **Output** drawer; status reads are not.
- Changes: Staged / Changes lists with status letters, +/− counts, the glyph chips of sessions
  that touched each file, "only this session's files", and an amber warning when a changed file
  was touched by more than one live session.
- Click a file for a side-by-side Monaco diff: stage / unstage / discard a hunk, or select lines
  and stage just those (`git apply --cached --recount`, built in `src-tauri/src/git.rs`).
- Commit box: **✦ Write with Claude** (`claude -p --model <setting>` on the staged diff), Amend,
  Commit ⌘↵, and Commit and push from the dropdown.
- History (200 commits, lane graph; right-click: copy hash, checkout, revert, cherry-pick,
  branch here), Branches (+ worktrees mapped to sessions), Stash.
- Fetch / Pull / Push run with a spinner and Cancel. Errors are rewritten into a sentence with a
  one-click fix where one exists (Set upstream and push, Pull, Stash changes…). Checkout, pull and
  rebase ask first when a session in that checkout is working.
- Conflicts (U) open with Accept current / incoming / both per conflict, Mark resolved, and
  "Ask a session to resolve".
- Status refreshes from a file watcher (300 ms debounce, ≤ 1/s) and after every action;
  auto-fetch every 5 min (setting).

### Editor

- ⌘E / ⌘2 opens Monaco (bundled, no CDN) in the center pane; the left panel becomes a file tree
  (git colours, session chips, ignored-files toggle, context menu incl. Move to Trash and
  "Send path to session…"). ⌘⇧1 goes back to the session list. ⌘\ from the editor puts the
  session on the left and the editor on the right.
- ⌘P quick open (git ls-files + untracked, fuzzy), ⌘⇧F find in files (ripgrep, falling back to
  git grep) with replace-all behind a confirmation, ⌃G go to line, ⌘S save, ⌘W close tab.
- Format on save (per project, off by default) uses the project's Prettier, Biome, Black or gofmt
  when its config exists.
- Gutter: added/changed/removed lines vs HEAD; lines a session changed are drawn in its colour
  (fleet snapshots a file on PreToolUse and diffs it on PostToolUse). Hover names the session.
- Agent edits reload silently when you have no unsaved changes; otherwise a bar offers
  Keep mine / Take theirs / Compare.
- ⌘I on a selection: Quick edit (one-off `claude -p`, shown as an inline diff, ⌘↵ accept / Esc
  reject), Send to the owning session, or New session. ⌘⇧I pastes the selection into the current
  session's prompt; ⌘⇧E opens the session's latest edited file.
- Language features through Monaco's LSP client, bridged to servers the backend runs over stdio
  (`src-tauri/src/lsp.rs`). TypeScript/JavaScript is built in (typescript-language-server is
  bundled; the project's own `node_modules/typescript` is preferred; needs `node` on PATH).
  pyright, gopls and rust-analyzer are used when found; others via Settings → Editor.

### Polish

- ⌘K palette: sessions, processes, files, branches and every command.
- **⌘N starts a session immediately** in the current project (the selected session's, else the most
  recent). ⌘⇧N opens the sheet for a first prompt, permission mode or **its own git worktree**
  (`.fleet/worktrees/<slug>` on `fleet/<slug>`, `.fleet/` added to `.git/info/exclude`). Closing a
  worktree session offers keep / merge & remove / delete worktree and branch.
- Quitting and reopening restores sessions (`claude --resume`), panel sizes and open editor tabs.
- Settings (⌘,) are stored in `~/Library/Application Support/fleet/settings.json`: theme, fonts,
  session colours, claude path / default args / permission mode / model / live titles, terminal,
  editor, git, notifications (mute per project or session, menu-bar item, sounds) and every
  shortcut, with a warning when two commands share keys.

### Themes

Settings → Appearance shows a gallery of live previews; ⌘K → "Theme: …" switches too. A theme changes colors, type,
shapes, depth and motion, and the terminals and editor follow.
- **Classic Dark / Classic Light**: the originals.
- **Hacker**: phosphor green on black, all mono, square, scanlines.
- **Cyberpunk**: signal yellow and neon cyan, chamfered edges, a glitch on state changes.
- **Glass**: a real see-through window (macOS vibrancy) with frosted panels.
- **Pear**: bright and native, with system type, segmented controls and one blue.
- **Retro-Futuristic**: indigo night, sunset accents, round shapes.
- **Soft**: neomorphism, with extruded controls and pressed selections.

**Effects** (full / subtle / off) tones down scanlines, glow and glitches, and macOS "Reduce motion" always wins.
Fonts are bundled, so themes work offline. Every theme is tested for text, status and chip contrast
(`tests/e2e/themes.spec.ts`, screenshots in `test-results/themes/`).

### Console (⌃N)

⌃N opens a console under the selected session: a login shell in the session's folder (its
worktree, else the project root), or on the server for SSH sessions. + adds shells, ⌃N again hides
it (or focuses it if focus is elsewhere). Consoles belong to their session and close with it.
⌃N is taken from the terminal (it would otherwise be \"next history line\" in shells and Claude Code); every other Ctrl key still goes to the terminal. Change it in Settings → Shortcuts.

### SSH connections

- Add them per project (Remote → "+ Add SSH connection"): an `~/.ssh/config` alias or user@host,
  the folder the code lives in on the server, optional port / identity / extra options.
- fleet uses your own `ssh` (config, keys, agent, known_hosts) with one ControlMaster per **server**:
  connections in different projects that point at the same host, port, identity and options share
  one connection (one login, one 2FA). Disconnecting one project's entry keeps the connection
  open while another project still uses it. Password / 2FA / host-key prompts appear in a login
  terminal in fleet; nothing is stored and nothing is installed on the server.
- Click a connection under Remote to open a shell on the server (connecting first if needed;
  clicking again brings back the same shell). Right-click → "New terminal" opens another one.
- Claude on a server: pick it under "Runs on" in the New session sheet (⌘⇧N), or right-click the
  connection → "New claude session". fleet connects first if needed (login terminal for
  password / 2FA). The session's tools run on the server via `fleet-remote-mcp`; the row shows ⇄.
- The git panel and editor work on local checkouts only; remote sessions show a note there.

### GitHub sync (optional, per project)

GitHub becomes the source of truth so you can continue on any Mac. Turn it on from the project's
right-click menu → **GitHub sync…** (checks git, `origin`, the GitHub CLI and sign-in, then creates `fleet.yml`).
With no `origin`, **Create GitHub repo** (private by default) makes the first commit on main if there's none (`.env`,
keys and huge files stay out), runs `gh repo create`, and pushes it with `ship::first_push`, the only main push outside
Ship, refused unless the GitHub repo is empty. Ship (⌘⇧D) in a project without sync opens this dialog instead of shipping
another project.

- **fleet.yml** (repo root, on main) holds processes, checks, setup/deploy commands and Claude settings. It's
  validated against `src-tauri/schema/fleet.schema.json`; errors show with line numbers in the git panel and fleet
  keeps the last valid version. Editing it in fleet commits to `fleet/config-<date>` and this Mac uses it right away
  ("config not deployed") until the next Ship. Env vars are names only; values come from your `.env` files.
- **Every session gets its own worktree** (detached at `origin/main`). When its first prompt finishes it gets a
  branch `fleet/<glyph>-<slug of title>`.
- **After every finished prompt** fleet does the following:
  1. Stages everything except `.env*` (`.env.example`/`.sample`/`.template` are kept), `*.pem`, `*.key`, `id_rsa*`,
     files over 5 MB, anything in `.gitignore`, symlinks that only work on this machine (absolute, leaving the repo,
     broken, or standing where an ignored folder would be) and nested repos that aren't submodules. Excluded files are
     flagged in the git panel. More than 3000 new files in one prompt pauses sync with where they are (usually
     dependencies or build output missing from `.gitignore`).
  2. Commits with a `claude -p` message plus the trailers `Fleet-Session`, `Fleet-Title`, `Fleet-Machine` and
     `Fleet-Prompt`. Hooks are skipped (`--no-verify`); checks run at Ship.
  3. Writes the session's file on the `fleet-state` branch, using git plumbing with no checkout.
  4. Pushes both in one atomic push. If GitHub rejects it, fleet fetches, replays the state commit or rebases the
     branch, and retries. A conflict marks the session "● needs you". Offline pushes queue and retry with backoff
     (5 s up to 5 min).
- Quitting or sleeping while a session is mid-prompt makes a `wip:` commit and pushes it (`sync.wipOnQuit`). Closing a
  session with uncommitted edits (an interrupted turn) does the same.
- fleet fetches at launch, every `fetchIntervalSec` (120 s), and when the window regains focus.
- Badges show ✓ pushed / ↑ n waiting / ⚠ conflict / ⏸ paused / ↓ behind GitHub (the worktree couldn't take newer
  commits). The status bar shows "✓ synced 20s ago", "offline · n waiting", or "⚠ GitHub: <project>" when GitHub
  refuses fleet's sign-in or access (never mistaken for offline); click it for the reason, the sync log, Sync now, or
  Pause. Unfinished syncs (crash, pause) are re-run, never relabeled as pushed.
- **main changes only through Ship.** Pushes go through `src-tauri/src/sync/pusher.rs`, which refuses
  main without a `ship::MainPush` token that only Ship can create. The git panel's Push is blocked on main in synced
  projects.
- Dev processes start in the selected session's worktree ("dev · PX"). "↻ in QX" moves a running one.

**Local work.** If a synced project's folder has uncommitted changes, unsynced commits or sessions from before sync,
the project shows *"Keep as sessions"*. Files a session edited move into that session's own worktree (its conversation
is moved too and carries on) on top of GitHub's main; everything else, plus commits GitHub doesn't have, becomes a
**Local changes** session. All of it is pushed, and the folder goes back to matching GitHub. Nothing is deleted: if a
step fails the folder's changes are put back, and fleet refuses when local main has commits a reset would strand.

**Other Macs.** After every fetch fleet reads `fleet-state`:
- Local sessions take newer titles, colours and recaps from their files.
- Files whose branch is gone are archived. A local session whose branch was shipped (here or on another Mac) runs a
  sync pass: with nothing after what Ship squashed (`shippedTip`), its worktree goes back to main; newer commits move
  to a new branch on top of main, so already-shipped work is never pushed or shipped again.
- A clean main folder is fast-forwarded, and dev processes restart if dependencies changed.
  If it can't be (local changes, another branch, own commits), a toast says it's behind, once per GitHub commit,
  since scripts run from it — a hand-run deploy too — would use the old code.
- Sessions with no local copy appear under **On other computers** with their machine and age.
- **Continue here** creates a worktree on the branch and starts claude primed with the recap, commits and diffstat. With
  the opt-in `sync.chatSync` (private repos only, secrets redacted) it resumes the same conversation instead.
- **Presence.** While a session is open, `refs/fleet/presence/<branch>` on GitHub names the Mac that has it. Every push
  carries a lease on it, so two Macs never push the same branch. Another Mac's fresh presence shows "active on iMac ·
  3m ago" with **Take over**, and closing a session releases it.
- **+ From GitHub** (sidebar) lists your repos (`gh repo list`), clones one, runs `sync.setup` and loads its sessions.
- Session worktrees (and Continue here) get the main folder's gitignored `.env*` files and an APFS clone of
  `node_modules` (only when the lockfiles match and the worktree's own rules ignore it), then `sync.setup`. When a
  fast-forward or rebase changes a lockfile, setup runs again and that folder's dev processes restart.
- A fleet.yml edit saved offline is pushed after the next fetch, and the local override is dropped once main has it.

**Ship (⌘⇧D)** lists branches that are done, pushed and ahead of main; settings changes and undo branches are included.
Tick and drag to set the order, then Ship:
1. In a temporary worktree at `origin/main` (`.fleet/deploy`; it gets the main folder's gitignored `.env*` files and an APFS clone of its `node_modules`, then `sync.setup` runs), each branch is merged (squash by
   default) and the fleet.yml checks are run. A conflict or failing check skips that branch: its session is set to
   "● needs you" and gets the output as its next message. For a conflict, an open idle session is asked to fix it
   right away; otherwise the session shows ⚠ conflict.
2. main is pushed fast-forward only, with a lease. If main moved, Ship starts over once. With `mainIsProtected`, a PR is
   opened and auto-merged instead (not merged within 30 min: auto-merge is turned off, nothing counts as shipped). If
   main isn't pushed, no branch or session is touched and the rows say "not shipped".
3. `deployCommand`, if set, runs at exactly the pushed commit in a tree reset to it (files a check rewrote don't leak),
   and only while it's still GitHub's main (a newer Ship from another Mac owns the deploy; logged `deploy_skipped`).
   Then `deploy-`/`ship-YYYYMMDD-HHMM` is tagged (pushed again by the next Ship if GitHub refused it) and
   `deploys.yml` gets an entry. Commands run in their own process group and stop after 45 min.
4. Merged branches are deleted on GitHub only while they still point at what was squashed; a branch that gained a
   commit during the Ship stays, and its new commits move to a new branch (see Other Macs). Their sessions continue
   on main. Other branches are rebased and force-pushed with a lease, by the Mac that holds them (`needsRebase`
   otherwise).

**Fix with Claude** (on a conflicting Ship row, or click a session's ⚠ conflict badge) hands the conflict to claude: the
branch's own session (prompted if open, resumed with `--resume` if closed or archived), else a new session on a worktree
of the branch with its identity from fleet-state (refused while another Mac holds it). Claude merges `origin/main` into
the branch, resolves, runs the checks and commits; fleet's normal push after the prompt is then a fast-forward, and the
branch can ship again. A rebase left behind by sync is aborted first. Only with `mergeStyle: rebase` does claude rebase
instead; that push replaces GitHub's branch with a lease on the commit it had (`SyncState.rewrite`, `src-tauri/src/sync/fix.rs`).

History offers **Redeploy previous** (the newest good deploy older than what's live; main is untouched), **Retry
deploy** when the last deploy failed or was skipped (runs it at GitHub's main and tags it), and **Undo** per feature
(a revert on a new branch that ships next time). The GitHub sync dialog warns when Actions run on every branch push and,
after showing the change, limits them to main and ship/deploy tags on a settings branch.

### Removing fleet's hooks

Nothing to undo in your Claude config. Quit fleet and delete
`~/Library/Application Support/fleet/` (state, UI layout, hooks file, socket).
Sessions you started in fleet remain resumable with `claude --resume <id>`.

## Notes and limits

- Notifications use UserNotifications (`UNUserNotificationCenter`) in the built `.app`
  (`dev.fleet.app`): macOS asks for permission on first launch, and fleet appears in
  System Settings → Notifications. Clicking one activates fleet and jumps to that session.
  Per-state sounds (Glass/Hero/Basso) are played with `NSSound`. Settings → Notifications →
  "Send test notification" reports whether macOS accepted it. In `tauri dev` (unbundled,
  where UNUserNotificationCenter isn't available) they fall back to the old API and are
  attributed to Terminal.app.
- Context % is read from the newest usage entry in the transcript; the window is 200k,
  or 1M when the model id says so or usage exceeds 200k.
- Titles cost one small `claude -p` call per finished turn (hooks disabled, no session
  persistence); turn them off in Settings → Claude.
- Go to definition can land on the import line for a second or two after opening a TS project,
  while tsserver loads it; after that it follows into the defining file.
- Notarization needs your Apple Developer ID (see release.sh); the default build is ad-hoc signed.

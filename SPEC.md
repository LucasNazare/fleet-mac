# Build prompt — fleet: a macOS terminal IDE for many Claude Code sessions
 · 

## How to use this
Paste everything below this section into Claude Code in an empty folder, or save it as SPEC.md and say "Build this, milestone by milestone, following SPEC.md". Ask it to stop after each milestone so you can try it before it moves on. The design reference is layout A on the design canvas.

## Role and goal
You are building fleet, a macOS desktop app that is a terminal and a lightweight IDE in one, designed for one developer running 10 or more Claude Code sessions in parallel across several projects.
The developer's problems today:
- They run Claude Code in many kitty windows and tabs and lose track of which window is which. Session names don't update and there are only a few colors.

- They can't see at a glance which sessions are waiting for them, working, or done.

- Dev servers (npm run dev and similar) are mixed in with the agent windows, though they belong to a project, not a session.

- For a one-line change they don't want to open VS Code and hunt for the file.

- They have no single place to see and manage git for a project while several agents change it.

fleet must solve all five. Every Claude Code session runs as the real claude CLI inside a real terminal (PTY), so nothing about Claude Code itself changes. fleet adds identity, state, navigation, git, dev-server management and a code editor around it.
Work in milestones (last section). After each milestone, run the app, check the acceptance criteria and stop for review. When a detail here conflicts with how Claude Code actually works today, check the current Claude Code docs, follow them and note the change.

## Tech stack and architecture
Build it as a Tauri 2 app: a Rust backend for PTYs, processes, git and the file system, and a React + TypeScript frontend for the UI. Target macOS 14+ on Apple Silicon first. Ship a signed .app and .dmg.
| Layer
 | Choice
 | Why
 |
| Shell
 | Tauri 2
 | Native window, small binary, Rust backend, macOS menus and notifications
 |
| Terminal emulation
 | xterm.js with the WebGL renderer, fit, search, web-links and unicode11 addons
 | Fast with 10+ live terminals, handles Claude Code's TUI
 |
| PTY
 | portable-pty crate
 | One PTY per session and per dev process, login shell env (zsh -l) so PATH, nvm and asdf work
 |
| Editor
 | Monaco (with its diff editor)
 | VS Code-grade editing and side-by-side diffs
 |
| Language features
 | LSP servers spawned by the backend (typescript-language-server, pyright, others by config), bridged to Monaco
 | Go-to-definition, hover, errors
 |
| Git
 | The git CLI, called from Rust with porcelain v2 output
 | Matches what the user and Claude run in the terminal exactly
 |
| File watching
 | notify crate, debounced, respecting .gitignore
 | Live git status and file-tree refresh
 |
| Search
 | ripgrep, bundled or found on PATH
 | Quick open and find in files
 |
| State store
 | Zustand on the frontend, JSON files in ~/Library/Application Support/fleet/
 | Simple and inspectable
 |
| UI font
 | JetBrains Mono everywhere, size set in settings
 | Terminal look
 |
Process model. The backend owns every PTY and child process. They keep running when you switch views, and their output keeps buffering (10,000 lines of scrollback per terminal by default). The frontend only renders the active terminal plus small previews. Terminal output goes to the frontend over Tauri channels in batched chunks, at most every 16 ms per terminal.
Event bus. One local Unix socket at ~/Library/Application Support/fleet/fleet.sock receives events from Claude Code hooks (see Sessions). The backend turns them into session state changes and pushes them to the UI.
Core data model (TypeScript names; mirror them in Rust):
- Project: id, name, rootPath, colorFamily, processes: ProcessDef[], defaultClaudeArgs.

- Session: id, projectId, claudeSessionId, glyph, color, title, titleHistory[], cwd, branch, worktreePath?, state, stateSince, pendingQuestion?, contextPct, filesTouched[], createdAt, pinned.

- SessionState: needs_permission | needs_reply | working | done | failed | idle | queued.

- ProcessDef: id, name, command, cwd, env, autoStart, autoRestart, port?.

- ProcessRun: defId, pid, status (running | exited | crashed | stopped), exitCode?, detectedPorts[], startedAt.


## Layout
One window, five regions, dark theme by default. The left and right panels can be resized by dragging and hidden (⌘B left, ⌘⌥B right). Sizes are saved.
- Top bar (34 px). App name, the active project, counts ("9 agents · 2 processes") and hints for ⌘K, ⌘N and ⌘E.

- Left sidebar (300 px default). A search field (⌘K) at the top. Below it, one collapsible group per project, sorted by most recent activity. Each group has two parts:
- Sessions: one row per Claude Code session, with the colored glyph chip, the live title (cut off with an ellipsis) and a state line such as "● needs you · run deploy?" or "◐ working · 11m". The selected row gets a background and a 3 px left bar in the session's color.

- Processes: the project's dev servers and other long-running commands, one row each, with status, port and uptime (see Project processes). Hidden when the project has none.

- Group header: the project name, session count, a + to start a new session in that project, and a small attention dot if any session in it needs you.


- Center pane. Shows one of three things, switched with ⌘1 / ⌘2 / ⌘3 or tabs at its top:
- Session: a header (40 px chip, title, project · branch · worktree · started 12:41, state with time in state, context meter), the live terminal running claude, and a 3 px top border in the session's color. The terminal takes the input directly. There is no separate input box, because Claude Code's own prompt is the input.

- Editor: see Editor.

- Process: the full terminal of a selected dev process.

- ⌘\ splits the center pane vertically so a session and the editor (or two sessions) sit side by side.


- Right panel (330 px default): Git. Git for the selected session's project. It replaces the diff preview from the mockup. See Git panel.

- Status bar (28 px). Counts across all projects ("● 2 need you · ◐ 4 working · ✓ 2 to review · ○ 1 idle"), each clickable to filter the sidebar. On the right: the current branch, ahead/behind counts and key hints.

Visual rules: background #0D1117, panels #12171E, borders #252D38, text #E2E8F0, muted text #94A0B0. All text must have a contrast of at least 4.5:1. State is always shown as a symbol plus a word, never by color alone: ● needs you (amber), ◐ working (blue), ✓ done (green), ✕ failed (red), ○ idle (grey), ◌ queued (grey).

## Sessions
A session is one claude process in its own PTY, tied to one project. fleet starts it, knows its Claude session id, and learns its state from Claude Code hooks, not by scraping the screen.

### Starting a session
- ⌘N opens a small sheet: project (defaults to the current one), optional first prompt, "Run in its own git worktree" (off by default), and permission mode (defaults to the project's setting).

- fleet creates a UUID and launches claude --session-id <uuid> --settings <fleet-hooks.json> [project defaultClaudeArgs] in the project folder, or in the worktree. If a first prompt was given, it is passed as the initial prompt argument.

- The PTY gets FLEET_SESSION_ID, FLEET_SOCKET and TERM=xterm-256color in its environment.

- Worktree option: run git worktree add <root>/.fleet/worktrees/<slug> -b fleet/<slug> and add .fleet/ to .git/info/exclude. The session row shows "worktree". Closing such a session asks whether to keep, merge or delete the worktree and branch.


### State from hooks
fleet writes fleet-hooks.json, which registers a command hook for each relevant event. The command is a tiny bundled binary, fleet-hook, that reads the hook's JSON from stdin, adds FLEET_SESSION_ID, writes it to the socket and exits at once with code 0. It must never block or change Claude Code's behavior. If the user's own hooks exist, fleet's hooks are added alongside them.
| Hook event
 | New state
 | Also
 |
| SessionStart
 | idle
 | Store transcript_path
 |
| UserPromptSubmit
 | working
 | Save the prompt for the title
 |
| PreToolUse / PostToolUse
 | working
 | Add edited file paths (Edit, Write, MultiEdit) to filesTouched
 |
| Notification (permission request)
 | needs_permission
 | Store the message as pendingQuestion, e.g. "run ./deploy.sh all?"
 |
| Notification (waiting for input)
 | needs_reply
 |  |
| Stop
 | done, or needs_reply if the last message ends with a question
 | Regenerate title and recap
 |
| SessionEnd
 | idle, or failed on a non-zero PTY exit
 |  |
Check the current Claude Code hooks documentation for exact event names and payload fields before building this, and use a dedicated permission event if one exists. Fallbacks: if no hook arrives within 3 s of new PTY output, keep the last state. A PTY that exits with a non-zero code is failed.

### Identity
Every session has a fixed identity that appears everywhere it does: sidebar row, center header, tab titles, the macOS window title, notifications, git panel tags and editor gutter marks.
- Color: from a 12-color palette chosen so no two live sessions share a color. Every color has text contrast of at least 4.5:1 on the background, and the colors differ in lightness, not only hue. The user can pin a color.

- Glyph: two uppercase letters from the title ("Pix fee labels" → PX), unique among live sessions. The user can pin a glyph.

- Live title: on each Stop, fleet runs claude -p --model haiku in the background with the last user prompt and the final assistant message from the transcript. It asks for a title of at most 6 words and a recap of 3 to 5 bullets. The title replaces the old one and the old one goes into titleHistory, shown as "was: …" in the header tooltip. The first title comes from the first prompt. A title edited by hand (double-click) stops auto-updating until reset.

- Context meter: the percentage of the model's context window in use. Read it from the newest usage entry in the transcript JSONL, or from the status line JSON if Claude Code provides it. At 80% and above the meter turns amber and the tooltip suggests /compact.

- Window title: ● PX · Pix fee labels — atrium-api for the focused session.


### Lifecycle actions
Right-click a session row, or use the command palette, to rename, pin color or glyph, duplicate (new session in the same project and branch), restart (claude --resume <id>), interrupt (send Esc), close (asks to confirm while working) and archive. Archived sessions are listed under "Archived" in each project and can be resumed.

### Restoring after relaunch
On quit, fleet saves every session's metadata. On launch, it reopens each non-archived session with claude --resume <claudeSessionId> in the same folder, and shows a short "Restored 9 sessions" toast. Sessions that were working come back as idle.

## Project processes
Dev servers, watchers and other long-running commands belong to a project, not a session. They live in the Processes part of each project group in the sidebar. They keep running while sessions come and go.
Defining them
- Saved per project in <root>/.fleet/project.json (processes: ProcessDef[]), so a team can commit it. The user can choose to keep it local through .git/info/exclude.

- The first time a project is opened, fleet reads the package.json scripts (and Procfile, docker-compose.yml, Makefile targets if present). It offers dev, start, watch and serve as one-click processes. It never starts anything without asking.

- "+ Add process" takes a name, command, working folder (relative to the project), env vars, auto-start on project open, and auto-restart on crash (with backoff: 1 s, 2 s, 4 s, up to 30 s).

Each process row
- ▶ npm run dev on the left. On the right: status and port, e.g. "● :5173 · up 2h", "✕ crashed (exit 1)" or "■ stopped".

- Ports are found by checking the process tree's listening sockets (lsof -iTCP -sTCP:LISTEN -P filtered by PID and its children) every 5 s. A detected port becomes a link that opens http://localhost:<port> in the default browser.

- Buttons on hover: start/stop, restart, open in center pane. Keyboard: select the row and press ↵ to open, ⌘R to restart, ⌘. to stop.

- A crash sends a macOS notification ("npm run dev crashed in atrium-api") and turns the row red.

The process terminal
- Opens in the center pane as a full terminal with its scrollback, a search bar (⌘F) and a Clear button. You can type in it, for example to answer a prompt from the dev server.

- Select error text and press ⌘⇧S, or use "Send to session…", to paste it into a session's prompt with a short header ("From npm run dev in atrium-api:"). The picker lists that project's sessions, newest first.

- Stopping sends SIGINT, then SIGTERM after 5 s, then SIGKILL after 10 s, to the whole process group, so no orphan servers keep ports busy.

- Quitting fleet asks whether to stop all running processes (default) or leave them running detached.


## Git panel
The right panel manages git for the selected session's project (or the selected worktree, when the session has one). Its header shows the branch switcher, ahead/behind counts ("↑2 ↓0") and buttons for Fetch, Pull and Push. It has four tabs.
1. Changes (default)
- Two lists, Staged and Changes, each with a count and "stage all" / "unstage all". Each file row shows its status letter (M, A, D, R, U), path and +/− counts.

- Each file row also shows the glyph chips of the sessions that touched it (from filesTouched). A filter at the top switches between "All changes" and "Only this session's files".

- Clicking a file opens Monaco's side-by-side diff in the center pane. In the diff, you can stage, unstage or discard a single hunk or selected lines.

- Overlap warning: when a changed file is also in another live session's filesTouched, an amber box above the list names both sessions ("feeExpenses.ts is also being changed by PX · Pix fee labels").

- Commit box at the bottom: a message field, a "Write with Claude" button that drafts the message from the staged diff (claude -p --model haiku), "Amend", and Commit (⌘↵). A dropdown on Commit offers "Commit and push".

- Discarding asks for confirmation and names the files.

2. History
- The last 200 commits of the current branch: short hash, message, author, relative time, plus a simple lane graph for merges.

- Clicking a commit lists its files; clicking a file opens its diff. Right-click offers copy hash, checkout, revert, cherry-pick onto the current branch, and create branch here.

3. Branches
- Local and remote branches with ahead/behind, last commit date and the worktree they are checked out in. Actions: checkout, new branch from here, merge into current, rebase current onto, delete (with confirmation), publish.

- The Worktrees list maps each worktree to its session's chip, with "Open session", "Merge into main" and "Remove worktree".

4. Stash
- List, apply, pop, drop, and "Stash changes" with a message.

Behaviour rules
- Every git action runs the git CLI in the project folder. The exact command and its output are logged in an "Output" drawer at the bottom of the panel, so nothing is hidden.

- Status refreshes on file-system events (debounced 300 ms) and after every git action. It never polls faster than once per second.

- Pull, push and fetch run with a spinner and can be cancelled. Errors (rejected push, merge conflict, missing upstream) appear as a readable message with a one-click fix where one exists, for example "Set upstream and push".

- Merge conflicts: conflicted files show a U badge. Clicking one opens a three-way view in Monaco with "Accept current / incoming / both" per conflict, plus "Ask a session to resolve", which sends the file list to a chosen session.

- Before checkout, pull or rebase, warn if a session in this checkout is working, because the agent's files would change under it.


## Editor
The editor is a real place to read and change code, so the user never has to open VS Code for small edits. It opens in the center pane (⌘E, or ⌘2) for the selected session's project.
File tree
- When the editor is open, the left sidebar switches to a file tree. ⌘⇧1 goes back to the session list; the editor stays open. The tree hides .gitignored files, with a toggle to show them.

- Changed files show their git status color. Files in a live session's filesTouched show that session's glyph chip, as in mockup D.

- New file, new folder, rename, delete (to Trash), reveal in Finder, copy path, and "Send path to session…".

Editing
- Monaco with tabs, the same dark theme, and JetBrains Mono. Syntax highlighting for at least TS/JS, JSON, CSS, HTML, Markdown, Python, Go, Rust, YAML, SQL and shell.

- ⌘P quick open: fuzzy search over git ls-files plus untracked files. ⌘⇧F find in files with ripgrep, results grouped by file, with replace-all behind a confirmation. ⌘G go to line.

- ⌘S saves. Formatting on save uses the project's Prettier, Biome, Black or gofmt when its config exists. Off by default, toggled per project.

- LSP features where a server is available: errors and warnings, hover, go to definition (F12), find references, rename symbol.

- Diff gutter: added, changed and removed lines compared with HEAD. Lines changed by a session are marked in that session's color, attributed from the PostToolUse file paths and the diff at that moment. Hovering the mark names the session.

Working alongside agents
- When a session changes a file open in the editor with no unsaved edits, the editor reloads it silently and keeps the cursor and scroll position. If the editor has unsaved edits, it shows a bar: "PX changed this file. Keep mine / Take theirs / Compare".

- ⌘I on a selection, or on the cursor line, opens an inline box (mockup D). The user types what to change and picks one of:
- Quick edit, the default. A one-off claude -p call with the file, the selection and the request. It shows the result as an inline diff to accept (⌘↵) or reject (Esc). It creates no session.

- Send to the owning session, if a live session has touched this file. Its prompt gets the file path, line range and request.

- New session in this project, started with that context as the first prompt.


- "Ask about this" (⌘⇧I) sends the file path and selection to the current session's prompt without submitting it, so the user can add to it.

- ⌘⇧E reveals the active session's latest edited file in the editor.


## Navigation, notifications and settings
Keyboard. Every app shortcut uses ⌘ (sometimes with other modifiers). All Ctrl combinations pass straight through to the terminal, because Claude Code and shells use them.
| Shortcut
 | Action
 |
| ⌘K
 | Command palette: jump to any session, process, file, branch or command by fuzzy search
 |
| ⌘J
 | Jump to the next session that needs you (oldest first)
 |
| ⌘] / ⌘[
 | Next / previous session in the sidebar order
 |
| ⌘⌥] / ⌘⌥[
 | Next / previous project
 |
| ⌘N
 | New session
 |
| ⌘1 / ⌘2 / ⌘3
 | Center pane: Session / Editor / Process
 |
| ⌘E
 | Toggle the editor
 |
| ⌘\
 | Split the center pane
 |
| ⌘B / ⌘⌥B
 | Hide or show the left / right panel
 |
| ⌘⇧G
 | Focus the git panel
 |
| ⌘P / ⌘⇧F
 | Quick open / find in files
 |
| ⌘I / ⌘⇧I
 | Inline edit / ask about this
 |
| ⌘⇧S
 | Send the selected text to a session
 |
| ⌘W
 | Close the editor tab, or ask to close the session
 |
| ⌘,
 | Settings
 |
All shortcuts can be changed in settings, which warn about clashes.
Notifications
- A macOS notification when a session moves to needs_permission, needs_reply, done or failed, and when a process crashes. It is skipped when that session is already on screen and the app is focused. The notification shows the glyph and title ("● PX · Pix fee labels: run ./deploy.sh all?"). Clicking it focuses that session.

- The Dock icon badge shows the number of sessions that need you.

- Optional: a menu bar item listing sessions by state, and a sound per state. Both off by default.

- Per project and per session: mute notifications.

Persistence. Saved in ~/Library/Application Support/fleet/: projects, sessions (with claudeSessionId, identity and title history), layout and panel sizes, open editor tabs per project, and settings. Writes happen at most every 2 s and on quit. Nothing is sent anywhere.
Settings (⌘,, also editable as settings.json)
- Appearance: theme (dark default, light), UI and terminal font and size, the session color palette.

- Claude: path to claude (found on the login shell's PATH by default), default arguments and permission mode, the model used for titles, recaps and commit messages (default haiku), and auto-titles on or off.

- Terminal: scrollback lines, cursor style, copy on select, option key as Meta.

- Editor: tab size, format on save, LSP servers by language.

- Git: show only the session's files by default, confirm before discard (always on), auto-fetch interval (default 5 min, 0 = off).


## Milestones and acceptance criteria
Build in this order. Stop after each milestone, list what was built and how to try it, and wait for the go-ahead.
- Shell and terminals. Tauri app, the five-region layout, projects added from a folder picker, sessions started as claude in real PTYs, switching between sessions.
- Done when: 10 sessions across 3 projects run at once; switching between them takes under 100 ms; Claude Code's TUI renders correctly (colors, spinners, the prompt, Ctrl keys); typing latency feels like kitty.


- State and identity. fleet-hook and the socket, the state table, colors and glyphs, live titles and recaps, the context meter, status bar counts, ⌘J, notifications and the Dock badge.
- Done when: a permission prompt in any session shows "● needs you" with the question within 1 s and sends a notification; a finished task gets a new title within 10 s; no two live sessions share a color or glyph.


- Project processes. Process definitions, auto-detected scripts, start/stop/restart, port detection, the process terminal, crash notifications, "Send to session".
- Done when: npm run dev started from fleet shows its port as a link; stopping it frees the port; a crash turns the row red and notifies.


- Git panel. Changes with hunk staging, session chips, the overlap warning, the commit box with "Write with Claude", history, branches, worktrees, stash, the output drawer.
- Done when: a full stage → commit → push cycle works without a terminal; every command run is visible in Output; a failed push shows a readable error.


- Editor. File tree, Monaco tabs, quick open, find in files, save, diff gutter with session colors, reload on agent edits, ⌘I with its three options, LSP for TypeScript.
- Done when: the user opens a file with ⌘P, changes a line and saves in under 10 s; a ⌘I quick edit shows an inline diff to accept; go to definition works in a TS project.


- Polish. Command palette, worktree sessions, restore after relaunch, settings UI, editable shortcuts, signed and notarized .dmg.
- Done when: quitting and reopening fleet brings back every session with --resume, the layout and the open tabs.


Quality bar for every milestone
- No blocking work on the UI thread. 10 busy sessions plus 3 dev servers keep the UI at 60 fps on an M1.

- Rust unit tests for the hook parser, the state machine, git porcelain parsing and port detection. Playwright tests for the main flows in the frontend.

- A README covering install, how hooks are set up, and how to remove them.

Out of scope for now: Windows and Linux, remote or SSH sessions, plugins, collaboration, and replacing Claude Code's own UI inside the terminal.


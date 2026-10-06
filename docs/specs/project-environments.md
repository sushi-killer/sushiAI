# Task: Project environments — configure once, run on any host

Owner request (2026-09-30): configure a project in one place — git remote, variables, secrets and
tokens, MCP servers — and run it on this Mac, or any SSH host with the same environment,
the way Codex Cloud / Cursor environments work, but on our own hosts. No re-entering access on
every machine.

Design: the owner's design concept; each task gets its location outside the repo. Rejected
variants are archived there.

## Owner decisions (recorded)

1. Standard path on every SSH host: projects live in `~/sushiai/<slug>`, created on first
   connect (not `/sushiai`: that needs sudo). A checkout found elsewhere is used, flagged
   "non-standard path".
2. The "+" (Add panel) popup is V4. Worktree is a checkbox (off by
   default). Host readiness pills in the host row. Claude account picker in the Claude Code row.
3. Multiple Claude subscriptions through `claude setup-token`; the account is picked per session.
4. New project is one window whose steps open in turn (Source → Hosts → Environment → Creating →
   Ready), not a multi-page wizard.

## Decisions (owner, 2026-09-30)

- **D1 · Secrets leave this Mac: decided (revised 2026-10-01).** To every SSH host the owner
  added: adding the host is the consent, so there is no consent screen and no per-host approval.
  The one control is "Don't send secrets to this host" (Project settings → Hosts, off by default,
  `hosts[host].withheld`); with it on the host gets no value of that project and its daemon drops
  what it holds. Values travel only in orchd memory, on the prepare script's stdin, or in the
  `session.create` `env` over the daemon protocol (through the ssh proxy for a remote host). There they
  live in the holder's environment and in the daemon's memory, and never in a command line, in
  `argv`, or in `state.json`.
- **D2 · Core changes: approved.** The owner started this feature knowing every lane touches
  `src/app/*`, `electron/*` or orchd.
- D3 · Project description in the app only (proposed) vs also a committable
  `sushiai.project.json` with no values. Not blocking; the second can be added later.

## Assumptions (lead, reversible)

- Project identity is `normalizeRemote(git remote)` (`src/app/useProjectGit.ts:14`). Without a
  remote it falls back to endpoint + cwd, as today. Existing workspaces attach to a project lazily
  the first time the project dialog opens.
- Stores: the `projects` table in `<userData>/sushiai.db` holds metadata and no values. Values go in
  its `project-secrets` store through `safeStorage`, the same code path as provider keys
  (`electron/model-providers.cjs:133`). The renderer never receives a value, only `hasValue`/`hint`.
- A Claude subscription token is passed through `CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR` (checked
  in CLI 2.1.285), never through env. If fd passing fails in a session, the fallback is env plus
  redaction. That is decided in S7 with a test.
- Remote values travel only over channels that leave nothing on the host: orchd in memory, or
  `session.create` `env` over the daemon protocol (the ssh proxy). They stay in the holder's
  environment and the daemon's memory; they are never in `argv` or in `state.json`.

## Out of scope

Subscription limit tracking and auto-switching accounts on limit. Docker / cloud executors. A shared
team project file (D3). An extension carrying secrets: this is core, not the manifest contract.

## Milestones

- **M1 · Local** (S1–S7): project, settings dialog, env/secrets/MCP injection, V4 popup,
  multi-subscription. Useful on its own on this Mac.
- **M2 · Remote** (S8–S10): hosts, the don't-send switch, delivery, prepare/clone, Run on.
- **M3 · New project** (S11), then **S12** docs, evidence and release on every milestone.

All sub-tasks are parts of one orchd feature and land on its branch; the feature ships as one PR
(`feat: project environments`) with a `docs/releases/unreleased/project-environments.md`
fragment.

## Sub-tasks

### S1 · Project entity and stores

- Project model (`id, name, git{url, defaultBranch}, env[], mcp{}, setup{install, check},
network{allowedDomains}, sessions{claudeAccount, backend}, targets[]`) in `src/types.ts`.
- `electron/projects.cjs`: CRUD, atomic writes, safeStorage values, IPC (`projects:*`). The value
  never crosses IPC.
- Workspace → project resolution by normalized remote.
- **Accept:** unit tests for resolution (ssh vs https remote, no remote, two hosts → one project);
  a secret round-trips through safeStorage and IPC exposes only the hint.
- Files: `src/types.ts`, `electron/projects.cjs` (new), `electron/ipc/*`, `electron/preload.cjs`.

### S2 · Project dialog shell + General (depends on S1)

- Replaces the `"workspace-actions"` dialog (`src/ClaudeMcpSettings.tsx`, `src/App.tsx:520`) with
  a rail: General · Environment · MCP servers · Hosts. Rename next to the name, Close project… at
  the bottom of the rail (keeps today's close flow).
- General: git remote, default branch, Sessions (Claude account default), Setup
  (install, check), Network (allowlist chips).
- **Accept:** everything the old dialog did still works (rename, close, refresh); `src/App.tsx`
  stays under 600 lines (`tests/app-boundary.test.cjs`); screenshots saved and compared with the design concept.

### S3 · Environment tab + .env import (depends on S1)

- Table: key, masked value + hint, available to (setup + agent · agent · setup only · MCP only),
  hosts (all / override).
- Import .env: parse, mark `*_TOKEN|*_KEY|*_SECRET` as secrets, review (exists/differs/new/same),
  "import as override" for conflicts, never keep the file.
- **Accept:** parser tests (quotes, `export`, comments, multiline, `=` in value); an imported secret
  is masked in the UI; screenshots saved and compared with the design concept.

### S4 · MCP servers tab (depends on S1)

- Project servers with `${VAR}` references (validated against Environment). Import `.mcp.json`.
- "Also in this project": the repo `.mcp.json` (read-only), `~/.claude.json` servers and Claude
  plugins with 30-day usage and on/off, ported from `ClaudeMcpSettings` (`electron/claude-mcp.cjs`).
- **Accept:** an unknown `${VAR}` shows an error; toggles still write `disabledMcpServers`;
  screenshots saved and compared with the design concept.

### S5 · Local delivery (depends on S1, S3, S4; after S7 in `terminals.cjs`)

- Local terminals and agents (daemon sessions, `electron/session-launch.cjs`) start with the
  project env for their stage. The setup-only values go only to the install step.
- Local orchd: `secrets.set` gains `projects: {id: env}` (`electron/orchestrator.cjs:603`,
  `orchd/src/engine/rpc_misc.rs`). `attempt_run.rs` adds the env to the agent process. orchd
  resolves `${VAR}` when it builds the run's `mcp.json`. Known values are redacted in logs and
  transcripts.
- **Accept:** orchd test: a secret is in the agent env, absent from `settings.json`, the run log
  and `tasks/*/mcp.json`; a setup-only value never reaches the agent; `cargo test` and
  `npm run ci` pass.

### S6 · "+" popup, V4 (depends on S1; the account picker uses S7)

- `src/app/PanelPickerDialog.tsx` redesigned. Head: project + switcher, environment pill. Host
  row: pills with readiness, New worktree checkbox (branch field on the path line).
  Two lists: agents (1–4), tools (letter keys) plus extension panels (`ExtensionPanelOptions`).
  Footer says where it starts; ⏎ starts the focused agent.
- **Accept:** keyboard (1–4, letters, ⏎, esc); the worktree and backend rules of today's code are
  kept (`launchesInWorktree`, SSH has no local worktree); extension panels still listed; desktop
  smoke; screenshots saved and compared with the design concept.

### S7 · Claude multi-subscription (depends on S1)

- `ClaudeAccount {id, label, kind: subscription|apiKey, hint}` in Settings → Providers. "Add
  subscription" opens a terminal pane with `claude setup-token`; the token is pasted into a masked
  field and stored like a provider key.
- Launch passes the token through the fd; API keys keep `apiKeyHelper`
  (`model-providers.cjs:278`). orchd routes get an `account` next to `profile`
  (`orchd/src/model.rs`).
- Project default (General → Sessions), per-session override in "+".
- **Accept:** two sessions on two accounts run side by side locally; `env` inside the session does
  not show the token; an orchd task uses the route's account; tests for fd passing.

### S8 · Hosts (depends on S1) — M2

- Hosts tab: readiness matrix (checkout, setup, CLIs, MCP, secrets) from the existing preflight
  (`electron/orchestrator-remote.cjs:87`) plus a checkout check against the project remote;
  gets secrets / no secrets; per-host overrides; the "Don't send secrets to this host" switch.
- `~/sushiai` is created on first connect; a non-standard checkout is detected and flagged.
- **Accept:** matrix states covered by tests with a fake ssh; the switch stops the next run from
  getting values and empties the host's orchd copy; screenshots saved and compared with the design concept.

### S9 · Remote delivery (depends on S5, S8)

- Remote orchd gets `secrets.set` for every host except those switched off for the project (drop
  the "never leave this machine" branch). SSH sessions get the values in `session.create` `env` over the daemon
  protocol (ssh proxy), held in the holder's environment and the daemon's memory, never in `argv` or
  `state.json`. Project MCP reaches remote tasks.
- **Accept:** live run on a remote SSH host: the agent sees the env, the host has no file left behind, a
  host switched off gets nothing; `docs/architecture.md` arrow updated.

### S10 · Prepare a host and Run on (depends on S8, S9)

- Prepare: clone into `~/sushiai/<slug>` (`GIT_ASKPASS` with the project's git token, or the
  host's own login), install when the lock-file hash changes, optional check.
- Orchestrator composer "Run on" menu with readiness (`src/orchestrator/HostSelect.tsx`); a
  host that is ready starts at once (a cheap readiness probe, no Prepare); Prepare shows its steps
  only when something is missing, uses an existing checkout of the repository where it is and clones
  only when there is none; a failure screen has actions (Try again, use the host's git login, edit
  token).
- **Accept:** a fresh host goes from "not cloned" to a running task in one flow; a 403 clone
  shows the failure screen and says what reached the host; screenshots saved and compared with the design concept.

### S11 · New project flow (depends on S1, S3, S8; remote clone uses S10) — M3

- Replaces `src/WorkspaceDialog.tsx`: one window with steps that open in turn. Step 1 Source (git
  URL / folder on a host / empty; reads branch, `.env.example`, `.mcp.json`, lock file) → Hosts
  (checkboxes; existing checkout reused) → Environment (fill secrets, MCP, install, Claude
  account) → Creating (per-host progress, runs in the background, reports to Inbox) → Ready
  (Start a session opens V4).
- Nothing from today's form is lost: name, folder, host; backend is the project default; the
  starter agent is chosen in "+".
- **Accept:** create from a git URL on This Mac + a remote SSH host; from an existing local folder; empty
  (`git init` in `~/sushiai/<slug>`); closing the window mid-create does not stop it; screenshots saved and compared with the design concept.

### S12 · Docs, evidence, release (every milestone)

- `docs/architecture.md` (project store, delivery arrows), `docs/INSTALL.md` (`~/sushiai`
  standard), release fragment per milestone, `docs/LESSONS.md` entry or an explicit "none".
- UI evidence through `.agents/skills/ui-evidence` for every changed screen, compared with the
  design concept; design-critic pass.
- **Accept:** `npm run ci` green; `npm run test:desktop` run; screenshots attached to each PR.

## Dependency graph

```
S1 ─┬─ S2
    ├─ S3 ─┐
    ├─ S4 ─┼─ S5 ─┐
    ├─ S6 ←┘(S7)  │
    ├─ S7         │
    └─ S8 ────────┴─ S9 ─ S10
S1 + S3 + S8 (+S10) ─ S11
S12 across all
```

Parallel lanes after S1: {S2}, {S3 → S5}, {S4}, {S6}, {S7}. No two lanes edit the same file:
S2 owns the dialog, S6 owns `PanelPickerDialog.tsx`, S7 owns Providers + fd launch in
`terminals.cjs`, and S5 edits `terminals.cjs` only after S7 lands.

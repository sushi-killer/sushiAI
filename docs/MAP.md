# Repository map

Every backticked path in this map is checked by `scripts/check-conventions.mjs`, so a moved or deleted file fails CI.

## Renderer

The React renderer lives in `src`. Its rules are in the renderer sections of `AGENTS.md` and `docs/architecture.md`.

- `src/main.tsx` is the entry file. `src/App.tsx` is the shell, capped at 600 lines by `tests/app-boundary.test.cjs`.
- `src/bridge.ts` is the typed wrapper over `window.sushiai`. `src/workspaceState.ts` holds workspace state. `src/styles.css` imports the styles.
- `src/app` holds the app pages and dialogs, such as `src/app/SectionPage.tsx` and `src/app/SettingsDialog.tsx`.
- `src/agents` holds the in-app Agents feature. Its doc is `docs/AGENTS-INTEGRATION.md`.
- `src/dialogs` holds shared dialogs.
- `src/extensions` holds extension loading, such as `src/extensions/modules.ts`. Its doc is `docs/extensions/companion.md`.
- `src/lib` holds small shared helpers.
- `src/mascot` holds the mascot window UI.
- `src/orchestrator` holds the orchestrator task UI. Its doc is `docs/orchestrator-settings.md`.
- `src/styles` holds the style sheets.
- `src/ui` holds shared UI components. The skill is `.agents/skills/sushiai-ui`.
- `src/vault` holds the vault UI.
- `src/workspace` holds workspace panels and layout.

## Electron

The main process lives in `electron`. Its rules are in `AGENTS.md` and `docs/architecture.md`.

- `electron/main.cjs` starts the app. `electron/preload.cjs` exposes the bridge to the renderer.
- `electron/ipc` handles IPC calls, one file per area: `electron/ipc/app.cjs`, `electron/ipc/chat.cjs`, `electron/ipc/daemon.cjs`, `electron/ipc/extensions.cjs`, `electron/ipc/projects.cjs`.
- `electron/extensions` loads extension manifests and companions. The manifest rules are in `electron/extensions/manifest.cjs`. The doc is `docs/extensions/companion.md`. The skill is `.agents/skills/extension-manifest-change`.
- `electron/agents` runs the in-app agents.
- `electron/daemon` talks to the Rust daemon.
- `electron/test-fixtures` holds fixtures for the Electron tests.
- `electron/app-db.cjs` holds app state. `electron/orchestrator.cjs` talks to orchd.

## Rust

The daemon and orchestrator live in `crates`. The crate rules are in `crates/ARCHITECTURE.md`. The dependency rule is run by `scripts/check-crate-deps.mjs`.

- `crates/sushiai` is the composition binary. Its entry is `crates/sushiai/src/main.rs`.
- `crates/sushiai-core`, `crates/sushiai-daemon` and `crates/sushiai-protocol` are the daemon core.
- `crates/sushiai-agents` runs agents.
- `crates/sushiai-hold` holds shared state for the daemon.
- `crates/sushiai-orch` is the orchestrator. `crates/sushiai-orch/src/lib.rs` exposes the host facade; `crates/sushiai-orch/src/engine/runtime.rs` owns `Orch::call` and runtime startup/shutdown.
- `crates/sushiai-orch/src/engine` is the task engine. Start at `crates/sushiai-orch/src/engine/mod.rs`.
- `crates/sushiai-orch/src/engine/lifecycle/run.rs` runs a task through lifecycle operations. `crates/sushiai-orch/src/engine/planning` plans.
- `crates/sushiai-orch/src/engine/verification` coordinates checks; `crates/sushiai-orch/src/verify` executes and classifies repository evidence. `crates/sushiai-orch/src/engine/review.rs` reviews.
- `crates/sushiai-orch/src/engine/land.rs` lands. `crates/sushiai-orch/src/engine/rpc` validates and dispatches task RPC.
- `crates/sushiai-orch/src/engine/attempts/brief.rs` builds the executor brief; `crates/sushiai-orch/src/engine/planning/prompt.rs` builds planning prompts. `crates/sushiai-orch/src/policy/settings.rs` has `Settings`.
- `crates/sushiai-orch/src/prompts/loader.rs` loads `crates/sushiai-orch/prompts/orchestrator.yaml`.
- `crates/sushiai-orch/src/task` owns pure task state and transitions; `crates/sushiai-orch/src/task/checks` owns criteria, commands, provenance, captured edits and baseline snapshots.
- `crates/sushiai-orch/src/harness` owns native provider argv and stream/session parsing; `crates/sushiai-orch/src/api` owns scoped MCP and event wire shapes.
- `crates/sushiai-orch/src/git` provides shared repository IO; `crates/sushiai-orch/src/delivery/git` owns commit, carry and saved-work recovery transactions.
- `crates/sushiai-orch/rust-standards.json` sets this crate's limits and dependency boundaries; the generic workspace check is `crates/sushiai/tests/rust_standards.rs`.
- Orchestrator settings are documented in `docs/orchestrator-settings.md`.
- The incomplete architecture checkpoint and all twelve follow-ups are in `docs/orchd-simplify-status.md`; measured ownership and remaining limits are in `docs/orchd-simplify-inventory.md`.

## Tests

- `tests` holds the Node tests, one test file per area.
- Rust integration tests are in the `tests` folder of each crate, such as `crates/sushiai-orch/tests`.
- `scripts/test-rust.mjs` runs the Rust tests. The skill is `.agents/skills/sushiai-testing`.

## Scripts

- `scripts/check-conventions.mjs` holds the repository rules. `scripts/check-crate-deps.mjs` holds the crate dependency rule.
- `scripts/release.mjs` cuts a release. Its notes helper is `scripts/lib/release-notes.mjs`. The skill is `.agents/skills/cut-release`.
- `scripts/hooks` holds the agent hooks: `scripts/hooks/guard-bash.mjs`, `scripts/hooks/check-touched.mjs`, `scripts/hooks/lesson-reminder.mjs`. They are wired in `.claude/settings.json`.
- `scripts/test-desktop.mjs` runs the desktop tests.
- `.github/workflows/ci.yml` runs `npm run ci` in CI.

## Skills

Skills live in `.agents/skills`. `.claude/skills` links to them. Roles live in `.claude/agents`.

- `.agents/skills/author-and-verify-extension` scaffolds and verifies an extension.
- `.agents/skills/autoreview` reviews a diff.
- `.agents/skills/cut-release` cuts a release.
- `.agents/skills/deslop` cleans a diff.
- `.agents/skills/extension-manifest-change` changes the manifest contract.
- `.agents/skills/ship-pr` opens a pull request.
- `.agents/skills/sushiai-task` is the task loop.
- `.agents/skills/sushiai-testing` picks proof.
- `.agents/skills/sushiai-ui` guides UI work.
- `.agents/skills/ui-evidence` measures and photographs the UI.

## Where is ...

- A setting: orchestrator settings are the `Settings` struct in `crates/sushiai-orch/src/policy/settings.rs`, documented in `docs/orchestrator-settings.md`. App settings UI is `src/app/SettingsDialog.tsx`, with state in `electron/app-db.cjs`.
- An IPC call handled: it starts in `electron/preload.cjs` and ends in a handler in `electron/ipc`.
- The orchd brief built: `crates/sushiai-orch/src/engine/attempts/brief.rs`. The planner prompt is `crates/sushiai-orch/prompts/orchestrator.yaml`.
- A CI rule: `scripts/check-conventions.mjs` and `scripts/check-crate-deps.mjs`, run by the `ci` script in `package.json`.
- One test: `node --test tests/<name>.test.cjs` for Node, `cargo test -p <crate> <filter>` for Rust.

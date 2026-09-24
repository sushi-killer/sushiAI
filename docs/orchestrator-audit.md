# Orchestrator end-to-end audit

Scope: `orchd` (the Rust daemon: task engine, orchestrator chat, verify/hook
gate, persistence, socket protocol), `orchd mcp` (the stdio MCP bridge),
`electron/orchestrator.cjs` (the Electron-side daemon client/lifecycle), and
`src/orchestrator/**` (the renderer UI). One confirmed bug was found and
fixed as part of this audit (below); everything else here is reported only.

## Confirmed bug (fixed): chat turns leak past daemon shutdown

**Where:** `orchd/src/engine.rs` (`App::shutdown`, `App::any_task_loop_running`)
and `orchd/src/main.rs` (the post-`serve_task` drain loop).

**What was wrong:** the daemon tracks two independent kinds of live child
process under two independent maps: `controls` (one `TaskControl` per
running task attempt, each holding a `CancelToken`) and `chat_turns` (one
`CancelToken` per repo with a live orchestrator-chat turn, `engine.rs:326`).
`App::shutdown()` — called from both the `shutdown` RPC
(`handle_shutdown`, `engine.rs:1063`) and the SIGTERM handler in
`main.rs:160` — only ever iterated `controls`:

```rust
pub fn shutdown(&self) {
    for ctrl in self.controls.lock().unwrap().values() {
        ctrl.cancel.cancel();
    }
    let _ = self.shutdown_tx.send(());
}
```

A chat turn's own `CancelToken` (created in `chat::handle_send`,
`chat.rs:145`) was never touched, so `chat::run`'s `tokio::select!` loop
(`chat.rs:417-453`) never took its `cancel.cancelled()` branch and never ran
`kill_group` on the harness child. Worse, `main.rs`'s post-shutdown drain
loop only polled `app.any_task_loop_running()` (`controls` again), so even a
chat turn that *did* get cancelled some other way had no guarantee the
daemon would wait for its child to actually die before the process exited
out from under it — unlike a task attempt, which had that guarantee.

The chat harness child is spawned with `setsid()`
(`chat.rs:394-400`, `pre_exec`), the same isolation task attempts use so a
killed daemon doesn't take its child down via a shared process group. That
isolation is exactly why an unmanaged chat child doesn't die with the
daemon — it keeps running, detached, with no parent left to reap it and (once
the daemon process has exited) no `CancelToken` anywhere in memory that
could ever cancel it. The only way out, pre-fix, was for the owner to find
and kill the orphaned `claude`/`codex` process by hand.

**Fix:** `App::shutdown()` now also cancels every live entry in
`chat_turns`; a new `App::any_chat_turn_running()` mirrors
`any_task_loop_running()`, and `main.rs`'s drain loop now waits on
`any_task_loop_running() || any_chat_turn_running()` before removing the
pidfile and exiting. `chat::run`'s existing `cancel.cancelled()` branch
already knew how to `kill_group` its child and return `RunError::Cancelled`
— it just needed to actually be triggered. No change was needed in
`chat.rs` itself.

### Reproduction steps (pre-fix behavior)

1. Build `orchd` and start it by hand with a harness binary that hangs
   without ever printing a `result` line (simulates a slow/stuck chat
   reply), e.g. `ORCHD_CLAUDE_BIN=/path/to/a/script/that/does 'sleep 30'`.
2. Call `chat.send` with `{"repo": "<some dir>", "text": "hello"}` over the
   daemon's unix socket.
3. Once the harness process has started (confirm with `ps` — it will be a
   new session leader, `setsid`'d), call `shutdown` over the same socket
   (or send the daemon process SIGTERM).
4. Pre-fix: the daemon process exits (the socket server and the main
   `select!` in `main.rs` both react to the shutdown signal correctly), but
   `ps` still shows the harness process running, now with no parent and no
   daemon left that could ever cancel it. It runs until its own workload
   finishes or an operator kills it by hand.
5. Post-fix: by the time `shutdown`'s caller sees the daemon process exit,
   the harness process is already gone (`kill(pid, 0)` fails). This exact
   sequence is now `orchd/tests/integration.rs`'s
   `shutdown_kills_a_live_orchestrator_chat_turn_s_child` test, using a fake
   harness script instead of a real `claude`/`codex` binary.

**Note — a related, still-open gap (not fixed here, out of scope):** task
attempts also survive an *unclean* daemon death (a crash or `kill -9`, not a
graceful `shutdown`/SIGTERM) because their pgid is persisted on the
`Attempt` and `recover_interrupted` (`store.rs:126`) SIGTERM/SIGKILLs any
stale pgid it finds at the next startup. `ChatThread` has no equivalent pgid
field and `recover_on_start` never looks at `chats/*.json` at all
(`engine.rs:471`), so a chat turn orphaned by an unclean daemon death (as
opposed to the graceful-shutdown path fixed here) still leaks forever, with
no recovery path even after a restart. This needs persisting the chat
child's pgid the same way attempts do, which is a larger change than the
task in scope here — recorded so it isn't lost, not fixed.

## Found while verifying this fix: orchd can't verify orchd

Two earlier attempts at this task failed `npm run test:orchd` with a
failure report that showed nothing but `Compiling`/`Checking` lines. Two
separate orchd problems explain that.

1. **Failure output hides the failing test (reported, not fixed).**
   `run_one_verify_command` (`engine.rs`) stores
   `tail_chars(stdout + stderr, 4000)`. Cargo prints test results on stdout
   and build progress on stderr, so when a cold build writes more than
   4000 characters to stderr, every test name and panic message is cut
   off. To reproduce: give a task `cargo test` as its verify command on a
   clean `target/`, with one failing test. The attempt's `failure.tail`
   contains no test name.
2. **Nested `sandbox-exec` (worked around in the tests only).** With the
   default `sandbox: native`, orchd wraps every verify command in
   `sandbox-exec`. The integration tests start their own daemons, and those
   daemons wrap their fake tasks' verify commands (`true`) in `sandbox-exec`
   as well. macOS refuses to apply a sandbox inside another one, so under
   an outer orchd verify every test that expects a task to reach `done`
   ends up `waiting` instead. This could not be confirmed here, because
   running `cargo` or `sandbox-exec` was denied in this environment. It is
   the only cause found that fits both earlier failures. The integration
   tests now probe `sandbox-exec` once (`native_sandbox_unavailable`) and
   switch their daemons to `sandbox: host` only when the probe fails, so
   they still exercise the real sandbox on a normal machine.

## Works / broken / half-done matrix

Legend: **Works** = correct by inspection and/or covered by a passing test.
**Broken** = a real bug or a documented gap with concrete user impact.
**Half-done** = implemented on the happy path only, or implemented but
unverified by any test.

### `orchd` — task engine, chat, verify/hook gate, persistence, socket protocol

| | Item |
|---|---|
| Works | Task attempt loop: tiering, failure-signature dedup, blocked-question routing, protected-path gate, review routing (`engine.rs`, extensively unit- and integration-tested). |
| Works | Task-attempt process-group cancellation and cleanup, both graceful (`shutdown`/SIGTERM → `any_task_loop_running` drain) and crash recovery (`store::recover_interrupted` kills stale pgids on restart). |
| Works | Control-token auth gating every method but `ping`/`hook.stop` (`engine.rs:458`, tested). |
| Works | `hook.stop`'s pure decision function (`hook.rs::decide_stop`) is well isolated and unit-tested independent of IO. |
| Works | Verify-command sandboxing (`sandbox-exec` on macOS) and diff-hash caching so an unchanged diff doesn't re-run verify twice. |
| Broken | A failed verify's `failure.tail` is the last 4000 chars of stdout followed by stderr, so a cargo build's stderr pushes the failing test out of the report (see "orchd can't verify orchd" above). |
| Broken | A verify command that itself runs orchd daemons (orchd's own `npm run test:orchd`) can't pass under `sandbox: native`: the inner daemons' `sandbox-exec` is refused inside the outer one. The integration tests now avoid this; the product behavior is unchanged. |
| Works (now) | Orchestrator chat turn cancellation on graceful shutdown/SIGTERM — **the bug fixed by this audit**; see above. |
| Broken | Orchestrator chat turn has no crash-recovery path: an unclean daemon death (crash, `kill -9`) still leaks its harness child forever, with no pgid persisted to clean it up on the next restart (see note above) — out of scope for this fix. |
| Half-done | `ChatThread.busy` is derived purely from `chat_turns` being non-empty in memory (`chat.rs:78`); a daemon that restarts always reports a leaked/orphaned thread as `busy: false` (idle) even before this fix's cancellation ran, since the new process starts with an empty `chat_turns` map regardless of what actually happened to the old child — there is no daemon-side signal distinguishing "cleanly finished" from "orphaned by a crash," only "was this process's map ever populated for this repo." |
| Half-done | `run_verify_cached`'s cache key is per-task-id only (`engine.rs`), shared across `hook.stop` calls and the post-session gate, but is never invalidated on daemon restart — a stale cache entry from a previous process could theoretically be read if the in-memory map somehow survived, though in practice a restart always starts with an empty cache so this is latent, not active. |

### `orchd mcp` (stdio MCP bridge, `orchd/src/mcp.rs`)

| | Item |
|---|---|
| Works | Synchronous, single-process-per-connection bridge translating `tools/call` into a control-token-gated socket round trip; deliberately excludes `task_delete`, `settings_set`, and `secrets_*` from the exposed tool surface (documented and correct). |
| Works | `initialize`'s `instructions` field carries the orchestrator role/guardrails text, keeping the agent from editing files or approving/stopping tasks itself. |
| Half-done | The bridge has no daemon-liveness handling of its own beyond whatever the underlying socket call returns — if `orchd serve` isn't running, every tool call simply fails with whatever IO error the socket connect produces; there's no friendlier "the orchestrator daemon isn't running" message surfaced to the calling agent. |

### `electron/orchestrator.cjs`

| | Item |
|---|---|
| Works | Daemon reuse/no-duplicate-spawn via `ping`-then-spawn with a single in-flight `starting` promise; rebuilt-binary detection waits for the old pid to exit before respawning on the same socket path; control-token re-read before every request. |
| Works | Method allowlist correctly excludes daemon-internal `hook.stop`/`shutdown` from renderer reach; reconnect-on-drop for the event `subscribe` stream uses `.unref()`'d exponential backoff so it never keeps the app alive. |
| Works | The daemon-outlives-the-app design itself is intentional and correctly implemented: `close()` only tears down this app's own subscribe socket, never the daemon. |
| Broken | The `OrchestratorService` instance returned by `registerOrchestratorExtension` is never stored or explicitly closed from `electron/main.cjs`'s `before-quit` handler, unlike sibling subsystems (`attention.close()`, `chatIpc.close()`, `terminalIpc.close()` are all called there) — harmless today only because process exit closes the socket anyway, but it's the same category of "shutdown path doesn't account for this subsystem's state" as the bug fixed in this audit, just with no user-visible consequence since Electron doesn't own the daemon's lifetime. |
| Broken | No daemon-crash signal reaches the renderer: if the daemon dies mid-task, the subscribe relay silently retries with backoff; nothing tells the renderer "the daemon restarted," so any assumption the UI made about in-flight state (in particular a chat thread's `busy` flag) can go stale with no visible indication until the next event for that same task/thread happens to arrive. |
| Half-done | `#pushSecrets` failures (after `connect()` and after `settings.set`) are swallowed with an empty `.catch()` — a failed `secrets.set` leaves the daemon on stale API keys with no user-visible error. |
| Half-done | No test exercises `registerOrchestratorExtension` itself or the `main.cjs` quit-path wiring (`tests/orchestrator.test.cjs` covers `OrchestratorService` in isolation only); no test covers a daemon restart happening while the subscribe socket is open mid-task. |

### `src/orchestrator/**` (renderer UI)

| | Item |
|---|---|
| Works | `helpers.ts`'s pure state-reduction functions (`applyOrchestratorEvent`, `upsertTask`, `statusLabel`, duration/cost formatting, log-line capping) are thoroughly unit-tested. |
| Works | Live task updates patch only the task whose id matches, scoped by repo; the blocked-question `QuestionCard` correctly disables while busy and supports both preset options and free text. |
| Broken | `OrchestratorChat` has no timeout/watchdog on `ChatThread.busy`: it renders "Thinking…" purely from the daemon-supplied `busy` field, fetched once on mount and otherwise only updated by a `chat` event pushed from the daemon. If a chat turn's daemon-side bookkeeping ever leaks across a restart without a terminal `chat` event for that thread (the crash-recovery gap noted above, or simply a repo that never reconnects), the panel is stuck on "Thinking…" indefinitely — no client-side timer, no "reconnected, this thread may be stale" banner, no re-fetch-on-reconnect. |
| Broken | No reconnect signal at all reaches the chat UI: `electron/orchestrator.cjs`'s subscribe relay reconnects transparently, so a full daemon crash+respawn is invisible to the renderer — `daemonState` is set once at mount and never re-derived from a live "disconnected" signal. |
| Half-done | `applyOrchestratorEvent` explicitly no-ops on `chat` events — chat state lives only in `OrchestratorChat`'s local `useState`, decoupled from the shared `live` reducer, so there is no single source of truth to reconcile against on reconnect. |
| Half-done | Zero component/integration tests for `OrchestratorPanel.tsx` or `OrchestratorSettings.tsx` — only `orchestrator-helpers.test.cjs` exists, testing pure functions; the stuck-"Thinking…" behavior above is unverified by any test, not just unhandled in code. |
| Half-done | Error surfacing is inconsistent: task-list/settings load failure sets a page-level `error`/`daemonState`, but a stuck chat only ever has `thread.error` (daemon-set) or a local `sendError` — nothing distinguishes "daemon still genuinely thinking" from "daemon dropped and forgot about this thread." |

# Lessons

A capped queue of open problems and fixed lessons. Check both before repeating
a mistake; `git log -p -- docs/LESSONS.md` preserves promoted entries and updates.

**Open** (not yet promoted to a real check/test/doc line): cap **8 entries,
120 words each** (including any `Update:` lines). **Whole-file cap: 600
words**, enforced by `scripts/check-conventions.mjs`. At either cap, promote
the oldest open entry or drop it with a one-line reason - never silently
delete.

Format for a new open entry:

```
## YYYY-MM-DD — <short symptom>
Root cause: ...
Rule: ...
```

Amend open entries with a dated `Update:` line. When promoted, compress to
one index line; git history keeps the detail and updates.

## Open

## 2026-10-15 — acknowledging a RAM-only amendment lost it after restart

Root cause: the live control held the pending edit while ordinary task saves knew nothing about it.
Rule: persist the pending receipt before acknowledgement; preserve it on ordinary writes and consume it with content-checked storage. Test a cold reload and a stale save, including child final-check contributions.

## 2026-10-15 — clearing a live amendment before publishing its plan lost the next edit

Root cause: baseline IO cleared the queue before publishing its admitted contract; the next accepted RPC captured stale content.
Rule: publish the plan and clear the queue under one lock before IO; guard results by full content identity. Persist relay carries before attempt admission; never call locking save helpers under that same lock.

## 2026-10-15 — a late save restored an owner-revoked waiver

Root cause: snapshots omitted owner answers and admitted stale decisions.
Rule: compare owner authority at publication; retain paid observations, then reload before choosing the next action. Test the real caller after rejection.

## Promoted

- 2026-10-14 New agent turns require matching fakes: parent landing route-review replay and `tests/common`.
- 2026-10-06 Build one real caller before generalizing: `AGENTS.md`, Core stays generic.
- 2026-10-07 Test daemons must isolate inherited homes: `tests/helpers/real-daemon.cjs`; isolation checks in `check-conventions.mjs`.

- 2026-10-14 PR delivery must carry onto the newest base and recheck before push: `a_done_task_whose_base_moved_is_carried_onto_the_new_base_before_the_push`.
- 2026-10-14 Fail-fast hid later baseline regressions → use `--no-fail-fast`. `tests/baseline_cargo.rs`.
- 2026-10-13 PR commits require a noreply author. `check-conventions.mjs`.
- 2026-10-13 Planner tests that failed the repo formatter stalled tasks → a repo `format` command runs before they are committed. `protected.rs`.
- 2026-10-13 Node 22 host: unref'd awaited timer failed `npm run ci` on main → `node-floor` CI job.
- 2026-10-12 Read-only git polls use `--no-optional-locks` to avoid index.lock.
- 2026-10-11 Checks starved at inherited background priority → `leave_background_priority`.
- 2026-10-11 Electron 44 has no postinstall; a fresh `npm ci` raced parallel tests into EEXIST → `install-electron` in our postinstall.
- 2026-10-10 An agent answer counted as the owner's → authority from the source. `an_mcp_accept_is_refused_and_mcp_continues_stop_at_the_cap`.
- 2026-10-07 Build before real-daemon tests; CI must reject skipped smoke. `tests/helpers/real-daemon.cjs`.
- 2026-10-07 A new session's first `hook.open` came before its actor and was refused → wait for every live session without an actor. `a_new_session_without_its_actor_is_waited_for_and_an_exited_one_is_not`.
- 2026-10-07 A restart clamped saved bounds to a small CI screen → hidden test windows skip saved bounds. `smoke.mjs`.
- 2026-10-07 Isolated test launchers must set `SUSHIAI_HOME`, `HOME` and `CODEX_HOME`. `check-conventions.mjs`.
- 2026-10-06 Agent-shell markers leaked into sessions → daemon strips them (lifecycle.rs test).
- 2026-10-06 Codex TUI hooks ran in a shared server → `--no-daemon` (launch_spec).
- 2026-10-02 Duplicate project rows survived three point fixes → data-model fix, not another patch.
- 2026-10-09 A reused LAN address sent the Hub `rsync --delete` to another server → check the target runs the service first. Remote `hub-deploy.mjs` preflight.
- 2026-10-02 Mid-session dead-row removal raced close flows → sweep at restore.

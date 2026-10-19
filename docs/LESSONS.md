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

## 2026-10-20 - a registry replacing a per-variant match widens a predicate

Root cause: one `ids()` was to replace the daemon's 4-name `AGENT_NAMES`, but the registry holds 5 agents; and a screen taken from `find(name)` would have let Claude read its screen, which only hook-less agents may do.
Rule: when a table replaces a `match`, name every predicate its arms encoded (`process_name()`, `agent()`, `screen()`), and pin the old truth table in a contract test.

## 2026-10-19 — orch tests wrote a hook into the machine's global hooks folder

Root cause: tests resolve `git rev-parse --git-path hooks`, which follows a global `core.hooksPath`; every repo on the Mac then ran the test hook.
Rule: test repos pin `core.hooksPath` to their own `.git/hooks` or run with `GIT_CONFIG_GLOBAL` set to a temp file.

## 2026-10-15 — a late save restored an owner-revoked waiver

Root cause: snapshots omitted owner answers and admitted stale decisions.
Rule: compare owner authority at publication; retain paid observations, then reload before choosing the next action. Test the real caller after rejection.

## Promoted

- 2026-10-15 A rejected continuation keeps its paid usage: `resume_fallback`.
- 2026-10-15 A live amendment's plan is published before its queue is cleared: `amend`.
- 2026-10-15 Acknowledged amendments survive restart: `an_acknowledged_live_wait_edit_survives_the_daemon_and_applies_after_restart`.
- 2026-10-14 New agent turns require matching fakes: parent landing route-review replay and `tests/common`.
- 2026-10-06 Build one real caller before generalizing: `AGENTS.md`, Core stays generic.
- 2026-10-07 Test daemons must isolate inherited homes: `tests/helpers/real-daemon.cjs`; isolation checks in `check-conventions.mjs`.

- 2026-10-14 PR delivery must carry onto the newest base and recheck before push: `a_done_task_whose_base_moved_is_carried_onto_the_new_base_before_the_push`.
- 2026-10-14 Fail-fast hid later baseline regressions → use `--no-fail-fast`. `tests/baseline_cargo.rs`.
- 2026-10-13 PR commits require a noreply author. `check-conventions.mjs`.
- 2026-10-13 Planner tests that failed the repo formatter stalled tasks → a repo `format` command runs before they are committed. `protected.rs`.
- 2026-10-13 Node 22 host: unref'd awaited timer failed `npm run ci` on main → `node-floor` CI job.
- 2026-10-16 Codex strict output rejected a loose result schema; fakes hid it → live smoke per harness, strict schema + `from_codex`. `a_codex_answer_is_normalised_to_v1`.
- 2026-10-11 Electron 44 has no postinstall; a fresh `npm ci` raced parallel tests into EEXIST → `install-electron` in our postinstall.
- 2026-10-10 An agent answer counted as the owner's → authority from the source. `an_mcp_accept_is_refused_and_mcp_continues_stop_at_the_cap`.
- 2026-10-07 Build before real-daemon tests; CI must reject skipped smoke. `tests/helpers/real-daemon.cjs`.
- 2026-10-07 A new session's first `hook.open` came before its actor and was refused → wait for every live session without an actor. `a_new_session_without_its_actor_is_waited_for_and_an_exited_one_is_not`.
- 2026-10-07 A restart clamped saved bounds to a small CI screen → hidden test windows skip saved bounds. `smoke.mjs`.
- 2026-10-07 Isolated test launchers must set `SUSHIAI_HOME`, `HOME` and `CODEX_HOME`. `check-conventions.mjs`.
- 2026-10-09 A reused LAN address sent the Hub `rsync --delete` to another server → check the target runs the service first. Remote `hub-deploy.mjs` preflight.

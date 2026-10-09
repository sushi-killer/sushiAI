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

## 2026-10-06 — a platform API grew before one real caller

Root cause: schema and fairness harness built before any real plugin used them.
Rule: land one working end-to-end path first, then generalize.

## 2026-10-07 — lanes from an old base rewrote the owner's `~/.sushiai` link

Root cause: JS tests on a base without the step 5 isolation changed the real home.
Rule: merge the isolation fix into every lane base before JS tests run.
Update 2026-10-07: inherited `SUSHIAI_HOME` relinked it again; spawned binaries set or remove it.

## Promoted

- 2026-10-07 Real-daemon tests skipped on CI (binary built after them) and hid Linux races → build first; they fail when `CI` is set. `tests/helpers/real-daemon.cjs`.
- 2026-10-07 A new session's first `hook.open` came before its actor and was refused → wait for every live session without an actor. `a_new_session_without_its_actor_is_waited_for_and_an_exited_one_is_not`.
- 2026-10-07 A restart clamped saved bounds to a small CI screen → hidden test windows skip saved bounds. `smoke.mjs`.
- 2026-10-07 A test launcher with only `SUSHIAI_HOME` rewrote the owner's `~/.codex` and bin link → set `HOME` and `CODEX_HOME` too. `check-conventions.mjs`.
- 2026-10-06 Agent-shell markers leaked into sessions → daemon strips them (lifecycle.rs test).
- 2026-10-06 Codex TUI hooks ran in a shared server → `--no-daemon` (launch_spec).
- 2026-10-02 Duplicate project rows survived three point fixes → data-model fix, not another patch.
- 2026-10-02 herdr ignored a lower `--seq` and repeats → signals carry a nonce. `open-signal.test.cjs`.
- 2026-10-02 Exact Herdr pin broke on self-update → check capabilities + protocol match.
- 2026-10-09 A reused LAN address sent the Hub `rsync --delete` to another server → check the target runs the service first. Remote `hub-deploy.mjs` preflight.
- 2026-10-02 Mid-session dead-row removal raced close flows → sweep at restore.
- 2026-10-02 Fixtures invented `gh` output → copy real CLI shapes (`MERGED`). `worktree-session.test.cjs`.
- 2026-10-02 Git identity loading split worktree rows → wait for first-host identities before drawing.
- 2026-10-01 A temp `$HOME` did not isolate Herdr on macOS → never stop an unowned server; test on SSH.
- 2026-10-01 Prepare lost SSH recovery between callers → preserve structured failures; reuse recovery UI. `GitRecovery` and `project-git-ssh.test.cjs`.
- 2026-10-01 contextBridge discards Error fields → transport tagged values; reconstruct before App restores state. `tests/bridge.test.cjs`.

- 2026-10-01 "0 unexplained diffs" hid defects → a fresh critic compares PNGs; revert each fix once to see its test fail. `design-critic`.
- 2026-09-30 orchd defaults leaked repo specifics to every repo → defaults stay empty. `scoped_checks_and_chat_tools_default_to_none`.
- 2026-09-30 A hook `allow` did not open `.claude/**` to headless `claude -p` → stage in `.orchd-staging/`, copy in before verify.
- 2026-09-30 A plugin MCP server failed under `--strict-mcp-config` (OAuth keyed by plugin name) → keep its Claude Code name. `chat_tools::server_key`.
- 2026-09-29 A kill lost uncommitted localStorage state → durable state goes in `sushiai.db`.
- 2026-09-30 A preselected answer plus Enter sent unpicked answers → Enter needs a pick or typed text. `enterAnswer` in `ownerAttention.ts`.
- 2026-09-28 Trusted `rtk rewrite` (`rtk read` drops code) → allowlist. `rtk_rewrite_output_*`.

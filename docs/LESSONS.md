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

## 2026-10-02 — duplicate project rows survived three point fixes

Root cause: no stored folder → project identity.
Rule: a subsystem regressing after point fixes gets a data-model fix, not another patch.

## Promoted

- 2026-10-02 herdr ignored a lower `--seq` and repeats → signals carry a nonce. `open-signal.test.cjs`.
- 2026-10-02 Exact Herdr pin broke on self-update → check capabilities + protocol match.
- 2026-10-02 Mid-session dead-row removal raced close flows → sweep at restore.
- 2026-10-02 Fixtures invented `gh` output (`CLOSED` + `mergedAt`), so merged cleanup never preselected → copy real CLI shapes (`MERGED`). `worktree-session.test.cjs`.
- 2026-10-02 Git identity loading split worktree rows → wait for first-host identities before drawing.
- 2026-10-01 A temp `$HOME` did not isolate Herdr on macOS → never stop an unowned server; test on SSH.
- 2026-10-01 Herdr 0.8.2 closed linked sessions with the primary's last pane → detach first. `herdr-pane-close.test.cjs`.
- 2026-10-01 Prepare lost SSH recovery between callers → preserve structured failures; reuse recovery UI. `GitRecovery` and `project-git-ssh.test.cjs`.
- 2026-10-01 contextBridge discards Error fields → transport tagged values; reconstruct before App restores state. `tests/bridge.test.cjs`.

- 2026-10-01 "0 unexplained diffs" hid defects → a fresh critic compares Figma and app PNGs; revert each fix once to see its test fail. `design-critic`.
- 2026-09-30 Lane worktrees left 262 MB of `orchd/target` → delete it after merge.
- 2026-09-30 orchd defaults leaked repo specifics to every repo → defaults stay empty. `scoped_checks_and_chat_tools_default_to_none`.
- 2026-09-30 A hook `allow` did not open `.claude/**` to headless `claude -p` → stage in `.orchd-staging/`, copy in before verify.
- 2026-09-30 A plugin MCP server failed under `--strict-mcp-config` (OAuth keyed by plugin name) → keep its Claude Code name. `chat_tools::server_key`.
- 2026-09-29 A kill lost uncommitted localStorage state → durable state goes in `sushiai.db`.
- 2026-09-30 A preselected answer plus Enter sent unpicked answers → Enter needs a pick or typed text. `enterAnswer` in `ownerAttention.ts`.
- 2026-09-29 Contradicting criteria cost three attempts → cross-check criteria before launch.
- 2026-09-28 Trusted `rtk rewrite` (`rtk read` drops code) → allowlist. `rtk_rewrite_output_*`.
- 2026-09-25 A changed orchd default never reached saved settings → mark settings differing from `settings.defaults`.
- 2026-09-25 Disabled `select!` branch still built `Instant + Duration::MAX`, panicking the loop → bounded deadline. `run_harness`.
- 2026-09-24 An unparsed review verdict counted as PASS → no verdict asks the owner. `a_review_without_a_verdict_waits_for_the_owner_instead_of_passing`.
- 2026-09-24 Chat harness outlived daemon shutdown → drain every live-child map. `shutdown_kills_a_live_orchestrator_chat_turn_s_child`.
- 2026-09-24 Orchestrator chat edited code instead of filing tasks → enforce roles with tools. `orchd/src/chat.rs`.
- 2026-09-24 Unset `HERDR_SOCKET_PATH`/inherited `BRIDGE_DEV_URL` photographed the owner's Herdr/dev server → override both. `ui-evidence` template.
- 2026-09-24 Real Claude/Codex output broke `orchd` while self-written fakes passed → parsers assert captured CLI lines. `parses_captured_real_cli_streams`.
- 2026-09-14 A debug paste leaked a real IP/host → examples use 192.0.2.0/24. `check-conventions.mjs`.
- 2026-09-29 orchd preflights checks on a fresh checkout → check commands build what they need. `package.json`.
- 2026-09-18 A fixture invented a Herdr status (`running`) → check vocabularies against `herdr api snapshot`. `tests/herdr-snapshot.test.cjs`.

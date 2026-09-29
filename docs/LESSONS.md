# Lessons

A short queue of open problems, plus an index of what's already been fixed.
Check both before repeating a mistake - but this file is not the archive;
`git log -p -- docs/LESSONS.md` is, so a promoted entry moves to the index
instead of staying around at full length forever.

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

While an entry is still open, amend it in place with a dated `Update:` line
instead of rewriting it, so its history stays visible. The moment it's
promoted, compress it to one line in the index below - the git history under
this file's own path keeps the discarded detail, `Update:` trail included.

## Open

## 2026-09-29 — A kill lost the whole workspace snapshot

Repro (old code, one data dir, layout change, 900 ms, SIGTERM to the `.bin/electron` wrapper like scripts/dev.mjs, relaunch): 3 panels before, 4 (defaults) after. Bare-binary SIGTERM kept state. Cause: Chromium had not yet committed localStorage.
Rule: durable state goes in `workspace-state.json`, atomically.

## Promoted

- 2026-09-29 Contradicting criteria cost three attempts → cross-check criteria before launch.
- 2026-09-28 A/B cause guessed from averages → read `events.jsonl` before explaining a result.
- 2026-09-24 `task.create` failed on an existing branch (`worktree add -b`) → new branch per task or omit `branch`.
- 2026-09-28 Trusted `rtk rewrite` (`rtk read` drops code) → allowlist. `rtk_rewrite_output_*`.
- 2026-09-25 A changed orchd default never reached earlier-saved settings (`settings.set` stores the whole struct) → mark settings differing from `settings.defaults`. `settings_defaults_reports_the_built_in_defaults_not_the_saved_settings`.
- 2026-09-25 Disabled `select!` branch still built `Instant + Duration::MAX`, panicking the loop → bounded deadline. `run_harness`.
- 2026-09-24 An unparsed review verdict counted as PASS → no verdict asks the owner. `a_review_without_a_verdict_waits_for_the_owner_instead_of_passing`.
- 2026-09-24 Chat harness outlived daemon shutdown → drain every live-child map. `shutdown_kills_a_live_orchestrator_chat_turn_s_child`.
- 2026-09-24 Orchestrator chat edited code instead of filing tasks → enforce roles with tools. `orchd/src/chat.rs`.
- 2026-09-24 Unset `HERDR_SOCKET_PATH`/inherited `BRIDGE_DEV_URL` photographed the owner's Herdr/dev server → override both. `ui-evidence` template.
- 2026-09-24 Real Claude/Codex output broke `orchd` while self-written fakes passed → parsers assert captured CLI lines. `parses_captured_real_cli_streams`.
- 2026-09-15 A `;`-joined gate chain let a failure commit → join gates with `&&`.
- 2026-09-15 Desktop smoke flaked: asserts followed fixed sleeps → wait for the state. `scripts/smoke.mjs`.
- 2026-09-15 Unit fixtures lacked the real layout's unrelated clone → grouping fixtures include a distractor. `tests/workspace-merge.test.cjs`, `tests/projects.test.cjs`.
- 2026-09-14 A `flex: 1`-stretched label measured 0px wide → measure text ink with a `Range`, assert non-zero widths. `ui-evidence` skill.
- 2026-09-14 A copied debug session leaked a real private IP/host/ports → examples use 192.0.2.0/24 (RFC 5737). `check-conventions.mjs`.
- 2026-09-13 Flat workspace list reordered on every poll → reconcile in place. `herdrSnapshot.ts`.
- 2026-09-13 A wrapper printed success for a run that exited 1 → exit code is the verdict, never `npx`. `sushiai-testing` skill.
- 2026-09-13 A user's ssh_config (`LocalForward`, `/dev/null` known-hosts) broke our tunnel → own `known_hosts`. `connections.cjs`.
- 2026-09-13 One global `connected` flag colored every host's status dot → per-endpoint `statusByEndpoint`. `useHerdr.ts`.
- 2026-09-12 Manifest duplicates/missing `surfaceId`, CI skipping test files, smoke dropping `SUSHIAI_EXTENSIONS_DIR` → `manifest.cjs`, contract-coverage test, `ci` script, extension skill.
- 2026-09-29 orchd preflights every verify/finalVerify on a fresh base checkout; `test:desktop` needed a prior build → check commands build what they need. `package.json`.
- 2026-09-16 Lane smoke failed on another lane's contract → land it first.
- 2026-09-18 A fixture invented a Herdr status (`running`) → check vocabularies against `herdr api snapshot`. `tests/herdr-snapshot.test.cjs`.

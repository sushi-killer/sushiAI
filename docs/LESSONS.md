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

## 2026-09-30 — Parallel lane worktrees filled the disk
Root cause: each `.claude/worktrees/agent-*` lane built its own `orchd/target`; ten lanes left 262 MB free.
Rule: delete a lane's `orchd/target` once it is merged; check `df` before more lanes.

## Promoted

- 2026-09-30 orchd defaults carried this repo's desktop smoke and one owner's MCP servers to every repo → defaults stay empty; repo specifics live in settings. `scoped_checks_and_chat_tools_default_to_none`.
- 2026-09-30 A hook `allow` did not open `.claude/**` to a headless `claude -p` (also not a `Write(.claude/**)` rule or `updatedInput`; checked on 2.1.285) → an allowed write goes to `.orchd-staging/` and is copied in before verify. `an_allowed_write_under_claude_goes_through_staging_and_lands_before_verify`.
- 2026-09-30 A connected MCP server that worked in `claude` failed under `--strict-mcp-config` (Slack's OAuth is keyed by the plugin name) → keep a plugin server's Claude Code name and probe with a real run. `chat_tools::server_key`.
- 2026-09-29 A kill lost the whole workspace snapshot (Chromium had not committed localStorage) → durable state goes in `workspace-state.json`, atomically.
- 2026-09-30 A preselected answer plus global Enter sent answers the owner never picked → Enter needs an explicit pick or typed text. `enterAnswer` in `ownerAttention.ts`.
- 2026-09-29 Contradicting criteria cost three attempts → cross-check criteria before launch.
- 2026-09-28 Trusted `rtk rewrite` (`rtk read` drops code) → allowlist. `rtk_rewrite_output_*`.
- 2026-09-25 A changed orchd default never reached earlier-saved settings (`settings.set` stores the whole struct) → mark settings differing from `settings.defaults`. `settings_defaults_reports_the_built_in_defaults_not_the_saved_settings`.
- 2026-09-25 Disabled `select!` branch still built `Instant + Duration::MAX`, panicking the loop → bounded deadline. `run_harness`.
- 2026-09-24 An unparsed review verdict counted as PASS → no verdict asks the owner. `a_review_without_a_verdict_waits_for_the_owner_instead_of_passing`.
- 2026-09-24 Chat harness outlived daemon shutdown → drain every live-child map. `shutdown_kills_a_live_orchestrator_chat_turn_s_child`.
- 2026-09-24 Orchestrator chat edited code instead of filing tasks → enforce roles with tools. `orchd/src/chat.rs`.
- 2026-09-24 Unset `HERDR_SOCKET_PATH`/inherited `BRIDGE_DEV_URL` photographed the owner's Herdr/dev server → override both. `ui-evidence` template.
- 2026-09-24 Real Claude/Codex output broke `orchd` while self-written fakes passed → parsers assert captured CLI lines. `parses_captured_real_cli_streams`.
- 2026-09-15 A `;`-joined gate chain let a failure commit → join gates with `&&`.
- 2026-09-14 A `flex: 1`-stretched label measured 0px wide → measure text ink with a `Range`, assert non-zero widths. `ui-evidence` skill.
- 2026-09-14 A copied debug session leaked a real private IP/host/ports → examples use 192.0.2.0/24 (RFC 5737). `check-conventions.mjs`.
- 2026-09-13 A wrapper printed success for a run that exited 1 → exit code is the verdict, never `npx`. `sushiai-testing` skill.
- 2026-09-29 orchd preflights checks on a fresh checkout → check commands build what they need. `package.json`.
- 2026-09-18 A fixture invented a Herdr status (`running`) → check vocabularies against `herdr api snapshot`. `tests/herdr-snapshot.test.cjs`.

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

## 2026-09-24 — Orchestrator chat harness leaked past daemon shutdown
Root cause: `App::shutdown()`/`main.rs`'s drain loop cancelled `controls`
(task attempts) but never `chat_turns`, so a `shutdown`/SIGTERM mid-chat
orphaned the child forever.
Rule: a new "live child" map needs both `shutdown()` and the drain-loop
wait, not just one. `orchd/src/engine.rs`, `orchd/src/main.rs`.

## Promoted

- 2026-09-24 Orchestrator chat edited code instead of filing tasks → enforce a role with tools, not prompt. `orchd/src/chat.rs`.
- 2026-09-24 Unset `HERDR_SOCKET_PATH` photographed the owner's real Herdr → missing path. `ui-evidence` template.
- 2026-09-24 Real Claude/Codex output broke `orchd` while self-written fakes passed → parsers assert captured CLI lines. `parses_captured_real_cli_streams`.
- 2026-09-15 A `;`-joined gate chain let a failing change commit → join gates with `&&`, pass file lists as explicit arguments.
- 2026-09-15 Desktop smoke flaked under load: asserts followed fixed sleeps → wait for the asserted state instead. `scripts/smoke.mjs`.
- 2026-09-15 Unit fixtures merged worktrees the real layout (main, worktree, unrelated clone) did not → grouping fixtures include a distractor. `tests/workspace-merge.test.cjs`, `tests/projects.test.cjs`.
- 2026-09-14 A `flex: 1`-stretched label measured 0px wide → measure text ink with a `Range`, assert non-zero widths. `ui-evidence` skill.
- 2026-09-14 A copied debug session leaked a real private IP/host/ports → examples use 192.0.2.0/24 (RFC 5737). `check-conventions.mjs`.
- 2026-09-13 Flat workspace list reordered on every poll: reconciliation appended each refresh at the array end, reshuffling other hosts → update in place. `herdrSnapshot.ts`.
- 2026-09-13 A wrapper printed "All files formatted correctly" for a run that exited 1 → exit code is the verdict, never `npx`. `sushiai-testing` skill.
- 2026-09-13 A user's ssh_config (`LocalForward`, `/dev/null` known-hosts) broke our tunnel → own `known_hosts`. `connections.cjs`.
- 2026-09-13 One global `connected` flag colored every host's status dot → per-endpoint `statusByEndpoint`. `useHerdr.ts`.
- 2026-09-12 Duplicate manifest labels broke a Playwright selector → `manifest.cjs`, `extension-contract-coverage.test.cjs`
- 2026-09-12 Commands without a `surfaceId` silently did nothing at runtime → AGENTS.md, `manifest.cjs`
- 2026-09-12 `webUtils.getPathForFile()` empty for dropped files (Electron 30-33 regression) → AGENTS.md
- 2026-09-12 Drag highlight flickered crossing a child element's boundary → AGENTS.md
- 2026-09-12 A copy-pasted duplicate in `ACTION_PLACEMENTS`/`ICONS` shipped silently → `extension-contract-coverage.test.cjs`
- 2026-09-12 CI silently never ran 33 of 52 test files → `package.json` `ci` script
- 2026-09-12 `scripts/smoke.mjs` discards an external `SUSHIAI_EXTENSIONS_DIR` → `author-and-verify-extension` skill
- 2026-09-13 Pushed a 31-commit branch to `main` unsquashed → squash-only is a `main`-history invariant. AGENTS.md.
- 2026-09-16 A lane's smoke failed on a bridge contract another lane held → land the contract commit first, give every lane its SHA, lead re-runs the smoke.
- 2026-09-18 A fixture invented an agent status Herdr never emits (`running`) → check a vocabulary against the running system (`herdr api snapshot`), not a fixture. `tests/herdr-snapshot.test.cjs`.

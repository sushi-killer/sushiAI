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

## 2026-09-14 — A layout check passed while the screen was wrong

Root cause: "tag 4px from the name" measured a box `flex: 1` stretched; a 23px button rendered 251px (`.workspace-panels button` out-specified it), squeezing a label to 0px.
Rule: measure the text ink (a `Range` over the text) and assert every label's rendered width is non-zero, then look at the shot.

## 2026-09-15 — Unit fixtures merged worktrees; the real layout did not

Root cause: "a host with two clones joins nothing" was checked per identity, so an unrelated clone beside a worktree pair vetoed the pair too. Every unit fixture held only the pair; the evidence repo set (main, worktree, clone) showed it at once.
Rule: a merge/grouping fixture includes a realistic distractor next to the members it expects to group.

## Promoted

- 2026-09-14 A test copied from a live debug session carried a real private IP, host alias and ports → examples use 192.0.2.0/24 (RFC 5737); private addresses fail CI. `check-conventions.mjs`.
- 2026-09-13 Flat workspace list reordered on every poll: reconciliation appended each refresh at the array end, reshuffling other hosts → update in place. `herdrSnapshot.ts`.
- 2026-09-13 A wrapper printed "All files formatted correctly" for a run that exited 1 → exit code is the verdict, never `npx`. `sushiai-testing` skill.
- 2026-09-13 A user's ssh_config (`LocalForward`, `/dev/null` known-hosts) broke our SSH tunnel → own `known_hosts`, private connection. `connections.cjs`.
- 2026-09-13 One global `connected` flag colored every host's status dot → per-endpoint `statusByEndpoint`. `useHerdr.ts`.
- 2026-09-12 Extension page opened in Agent/Chat mode left the sidebar blank → AGENTS.md, `toggleSection`/`src/app/navigation.ts`
- 2026-09-12 Duplicate manifest labels broke a Playwright selector → `manifest.cjs`, `extension-contract-coverage.test.cjs`
- 2026-09-12 Commands without a `surfaceId` silently did nothing at runtime → AGENTS.md, `manifest.cjs`
- 2026-09-12 `webUtils.getPathForFile()` empty for dropped files (Electron 30-33 regression) → AGENTS.md
- 2026-09-12 Drag highlight flickered crossing a child element's boundary → AGENTS.md
- 2026-09-12 A copy-pasted duplicate in `ACTION_PLACEMENTS`/`ICONS` shipped silently → `extension-contract-coverage.test.cjs`
- 2026-09-12 Smoke test failed ("expected 1, got 2") after deleting a fixture → `scripts/smoke.mjs`
- 2026-09-12 `types.ts` drifted from the manifest validator mid-session → `extension-manifest-change` skill
- 2026-09-12 Import-isolation check caught one import style → `check-conventions.mjs`
- 2026-09-12 CI silently never ran 33 of 52 test files → `package.json` `ci` script
- 2026-09-12 `scripts/smoke.mjs` discards an external `SUSHIAI_EXTENSIONS_DIR`; manifest fields are non-obvious → `author-and-verify-extension` skill
- 2026-09-13 Pushed a 31-commit branch to `main` unsquashed → squash-only is a `main`-history invariant. AGENTS.md.

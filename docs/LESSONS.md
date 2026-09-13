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

(none right now)

## Promoted

- 2026-09-12 Extension page opened in Agent/Chat mode left the sidebar blank → AGENTS.md, `toggleSection`/`src/app/navigation.ts`
- 2026-09-12 Duplicate manifest button labels broke a Playwright selector → `manifest.cjs` validator, `extension-contract-coverage.test.cjs`
- 2026-09-12 Commands without a `surfaceId` silently did nothing at runtime → AGENTS.md, `manifest.cjs`
- 2026-09-12 `webUtils.getPathForFile()` returned empty paths for drag-and-drop files (known Electron 30-33 regression, not an app bug) → AGENTS.md
- 2026-09-12 Drag highlight flickered crossing a child element's boundary → AGENTS.md
- 2026-09-12 A copy-pasted duplicate in `ACTION_PLACEMENTS`/`ICONS` shipped silently → `extension-contract-coverage.test.cjs`
- 2026-09-12 Smoke test failed ("expected 1, got 2") after deleting a fixture → `scripts/smoke.mjs`
- 2026-09-12 `types.ts` drifted from the manifest validator mid-session → `extension-manifest-change` skill
- 2026-09-12 The shell/extension import-isolation check only caught one import style → `scripts/check-conventions.mjs`
- 2026-09-12 CI silently never ran 33 of 52 test files → `package.json` `ci` script
- 2026-09-12 `scripts/smoke.mjs` silently discards an external `SUSHIAI_EXTENSIONS_DIR` → `author-and-verify-extension` skill, `extension-builder` agent
- 2026-09-12 The manifest schema has several required-but-non-obvious fields → same as above
- 2026-09-13 `lesson-reminder.mjs`'s single-signal heuristic (a tool error, or 3+ edits to one file) couldn't tell deliberate diagnostic noise or planned multi-step work from real struggle, and re-fired on stale transcript history - false-positived repeatedly in real use, including 8+ blocks in one long session → rebuilt as two stages: a per-turn heuristic pre-filter, then a small model judging a bounded transcript digest against four explicit criteria; only its verdict blocks. `scripts/hooks/lesson-reminder.mjs`, `.claude/settings.json`.

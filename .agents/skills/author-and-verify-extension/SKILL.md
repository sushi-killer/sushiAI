---
name: author-and-verify-extension
description: Builds a new sushiAI extension end to end: designs the manifest, validates it with validateExtensionManifest, builds, and proves it loads in a real Electron smoke run against a throwaway extensions directory. Use when asked to add, scaffold or build an extension, plugin or surface.
context: fork
agent: general-purpose
---

# Author and verify an extension

This is the concrete answer to "I want to write a plugin" — the workflow a
contributor actually runs, not just a manifest reference. See
`AGENTS.md`'s "Extension contract is the capability surface" section for the
contract facts cited below; this skill is the procedure for exercising it.

## The loop

1. **Design the manifest shape** from what was asked: which surfaces
   (with `instancePolicy`, `stateScope`, a `view.document` describing fields
   and views), which `navigation`/`actions` entries expose it, which
   `commands` open it. Constraints from the contract: `MAX_VIEWS = 1` (only
   `views[0]` renders), `commands[].surfaceId` is required, labels on
   `navigation`/`actions` must be unique within the manifest, `editable`
   fields must already exist in that view's `meta`.

2. **Write the manifest to a throwaway directory** — e.g.
   `/tmp/sushiai-ext-<name>/manifest.json` — never into
   `tests/fixtures/extensions/`. That directory is a tracked fixture set;
   `scripts/smoke.mjs` asserts its entry count matches exactly what's
   checked in, so dropping a scratch extension there fails an unrelated
   assertion.

3. **Validate it** before running anything: `validateExtensionManifest`
   from `electron/extensions/manifest.cjs` (Node, `require`d directly)
   against `{ ...manifest, source: { kind: "local", path: "<the throwaway dir>" } }`.
   Fix every thrown error before moving on — the validator's message names
   the exact field.

4. **Build, then launch against the throwaway extension with a driver script
   you write, not `scripts/smoke.mjs` itself**: `npm run build` first (the
   app under test is the built bundle - a stale `dist/` silently tests the
   previous build). `scripts/smoke.mjs` cannot be redirected by setting
   `SUSHIAI_EXTENSIONS_DIR` in your shell - it unconditionally overwrites
   that key to the literal `"tests/fixtures/extensions"` when it spawns
   Electron (`env: { ...process.env, SUSHIAI_EXTENSIONS_DIR: "tests/fixtures/extensions" }`),
   so an external value is silently discarded. Instead, copy
   `scripts/smoke.mjs`'s own setup (the build-freshness check and its
   `electron.launch(...)` call, first ~75 lines) into a throwaway script,
   point _your copy's_ `SUSHIAI_EXTENSIONS_DIR` at the throwaway directory,
   and replace its probe-specific assertions with assertions about your own
   extension (its nav label appears, its surface opens without a renderer
   error).

5. **Report pass/fail** with what was verified: the extension loaded, its
   surface opened, and (if state is involved) a write round-tripped.

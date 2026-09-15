---
name: extension-manifest-change
description: Changes what an extension manifest may contain: validator in electron/extensions/manifest.cjs, then the mirrored src/extensions/types.ts, then the probe fixture and contract-coverage tests, in that order. Use for any request to add, rename, narrow or remove a manifest field, or make the validator accept or reject something.
---

# Extension manifest contract change

Four steps, always in this order. Skipping the last two is how the validator
and the type system drift silently mid-session.

1. **Edit the validator** in `electron/extensions/manifest.cjs`. The closed
   sets a manifest may draw from (hosts, placements, icons, field types, ...)
   are collected once in the `CONTRACT` object at the bottom of the file —
   add a new closed set there too, not just inline in the validator function.
   Known shape constraints worth checking against: `MAX_VIEWS = 1` (the
   renderer only ever draws `views[0]`), contribution `id`s must be unique
   per-manifest per-kind (`surfaces, navigation, actions, commands`), labels
   on `navigation`/`actions` must be unique too, `commands[].surfaceId` is
   required (never make it optional again).

2. **Sync `src/extensions/types.ts`** to match. The validator and the type
   file are two independent copies of the same contract; nothing fails
   automatically if only one changes.

3. **Update the tests**: `tests/extension-manifest.test.cjs` (validator
   behavior — accepted and rejected shapes) and
   `tests/extension-contract-coverage.test.cjs` (which asserts, among other
   things, that every value in `CONTRACT` is exercised by
   `tests/fixtures/extensions/probe/manifest.json`, and that `types.ts`
   lists the same values as the validator for the sets it mirrors). Widening
   the contract without touching the probe fixture fails this test on
   purpose — widen the fixture too.

4. **Run `npm run check:conventions`**, then the full `npm run ci` before
   calling the change done.

# AGENTS.md

Instructions for any agent (Claude Code, Codex, or a human) working in this
repository. `CLAUDE.md` is a one-line pointer to this file — this is the only
copy of the rules.

## Enforcement principle

CI (`npm run ci`, which chains the build, the full test suite,
`scripts/check-conventions.mjs`, lint and format checks) is the only
enforcement layer every agent and every human shares. `.claude/hooks` and
`.claude/skills` (and their Codex equivalents under `.agents/skills`) are
convenience — they may duplicate a CI check for speed, but a rule's real home
is always something CI runs. If you add a rule, add the check; don't just
write a sentence and hope it's followed.

## Design priorities

- The extension contract (manifest + shell/extension separation +
  singleton-surface pane resolution) is the one piece of architecture every
  other decision should route through. See "Extension contract is the
  capability surface" below.
- Conversation/session state and navigation state are kept separate from
  extension state; extensions never reach into shell internals to read either.

## Extension contract is the capability surface

Prefer extending `src/extensions/*` / `electron/extensions/*` over touching
shell or core. This is deliberate, not incidental — the contract exists so a
third-party extension can't destabilize the shell:

- **Shell isolation**: every `src/app/*.tsx` file except `SectionPage.tsx`
  may import only `../extensions/{ExtensionSlots.tsx,registry.ts,routes.ts,types.ts}`
  (`scripts/check-conventions.mjs`) and must not reference
  `SurfaceRenderer`/`activePage`/`resolveNavigation` in their source — only
  `src/app/SectionPage.tsx` may draw a contributed page.
- **Manifest contract** (`electron/extensions/manifest.cjs`): every
  `commands[]` entry requires a `surfaceId` (a command that names no surface
  has nothing to do — don't reintroduce it as optional); `MAX_VIEWS = 1`, the
  renderer only ever draws `views[0]`; an `editable` field must already exist
  in that view's `meta`, and only `select`/`date` field types get editors;
  contribution `id`s must be unique per-manifest per-kind
  (`surfaces, navigation, actions, commands`), and so must `navigation`/
  `actions` **labels** — a duplicate throws (`Duplicate extension ${kind}
label`), because two identical labels break `getByRole` selectors in
  tests.
- **Extension API versioning**: `CONTRACT.SUPPORTED_API_VERSIONS`
  (`electron/extensions/manifest.cjs`) is the compatibility contract for
  third-party extension authors, versioned independently of sushiAI's own
  app release version — it's a single integer, not semver, because the
  manifest schema doesn't need a compat matrix. An additive change (a new
  optional field, field type, icon, or placement) never bumps it. Removing,
  renaming, or re-meaning something does: add the new version to the array,
  keep accepting the old one for at least two app minors (with a
  deprecation warning), then drop it. Every `docs/releases/<version>.md`
  states the `Extension API:` version(s) it ships with, generated from that
  same constant — never hand-maintain a separate compat table.
- **Navigation**: `toggleSection` (`src/app/navigation.ts`) forces
  `mode: "Code"` when opening an extension page, because Agent/Chat modes own
  the sidebar and would go blank if a page opened there; closing a section
  leaves `mode` untouched, implicitly restoring whatever was active before.
- **Ordering**: `compareOrder` (`src/extensions/registry.ts`) is the single
  sort used for both `navigation` and `actions` contributions —
  `order` first, then `extensionId.localeCompare`, then `id.localeCompare`.
  Don't write a second ordering function; extend this one.
- **Singleton surfaces**: enforced in `resolvePaneOpen()`
  (`src/extensions/routes.ts`) — a surface declaring
  `instancePolicy === "singleton"` reveals its existing pane instead of
  opening a second one. This is a routing-layer guarantee, not a renderer
  one; `SurfaceRenderer`/`ExtensionSlots` just draw whatever pane they're
  given.
- **Pure helpers**: record filtering/sorting/display logic lives in
  `src/extensions/records.ts` (`seedForFilter, text, truthy, display, toneOf,
compareRecords, bucketOf, orderBuckets`). `relativeDate`/`dueBucket` are
  deliberately kept module-internal — don't export them "for reuse" without a
  second caller that actually needs them.

## One owner, complete cutover

When a domain moves (the Tasks → Probe fixture migration is the template),
remove the old path entirely in the same change. Don't leave a compatibility
shim, a re-export, or an "old and new" fork — one owner, one final shape.

## Runtime and code safeguards

`scripts/check-conventions.mjs` already enforces, in CI:

- No Cyrillic in shipped source (`src/`, `electron/`).
- No raw hex colors in `src/styles/**/*.css` outside `src/styles.css` /
  `src/styles/tokens.css` — spend tokens, don't hardcode colors.
- The shell/extension import isolation described above.
- No AI-attribution commit trailers (`co-authored-by:`, "generated with
  claude", etc.) and no Cyrillic in commit messages.

On top of that: `npm run lint` (ESLint, `eslint.config.mjs`) catches
React-hooks misuse (`rules-of-hooks`, `exhaustive-deps`) and general
TypeScript issues; `tsc --noEmit` runs with `strict: true` plus
`noUnusedLocals`, `noUnusedParameters`, `noFallthroughCasesInSwitch`;
`prettier --check` enforces formatting. All of these are steps in
`npm run ci` — don't treat any of them as optional.

## Security boundary

Extensions are declarative `manifest.json` only, size-capped
(`electron/extensions/local-extensions.cjs`, `MAX_MANIFEST_BYTES`). The
renderer runs with `contextIsolation: true`, `sandbox: true`,
`nodeIntegration: false` (`electron/main.cjs`). This is the actual security
model for third-party extensions: they describe UI and data, they never ship
executable code. Any task that would have an extension carry its own JS/native
code breaks this invariant — stop and ask before implementing it, don't just
build it because it's technically possible.

## Vision & boundaries

sushiAI is meant to be a plugin-extensible platform for AI-agent workflows:
open-source, customizable, with a monetized core. As understood today:

- `src/extensions/*` and `electron/extensions/*` (the contract surface) are
  free to extend — that's the whole point of the architecture.
- Structural changes to `src/app/*`, `electron/main.cjs`, or the manifest
  validator itself are closer to the monetized core — ask before changing
  these, don't assume.

This boundary will get more precise as monetized surfaces actually get built;
don't invent detail beyond what's stated here.

## Product and validation — Definition of Done

- `npm run ci` is green.
- Desktop smoke (`npm run test:desktop`) was run if `src/app`,
  `src/extensions`, or `electron/` changed.
- UI changes were actually looked at (a screenshot, or a description of what
  was visually checked) — green tests do not prove a screen looks right.
- A `docs/releases/unreleased/*.md` fragment exists for any user-visible
  change (see "Release and branching" below).
- `docs/LESSONS.md` has a new entry, or the session explicitly states there
  was no lesson worth recording.

## Release and branching

Merges are squash-only: a PR's title becomes the literal commit message on
`main`, which is what `scripts/release.mjs` reads to compute the next
version. This makes the branch prefix and PR title a real contract, enforced
by `scripts/check-conventions.mjs` in CI, not just a naming convention:

- Branch names: `feature/<slug>` (pairs with a `feat: ` title),
  `fix/<slug>` (pairs with `fix: `), `chore/<slug>` (pairs with anything
  else — docs, refactor, test, ci, perf, chore), `release/<slug>` (reserved
  for `scripts/release.mjs`, pairs with `chore(release): `).
- A breaking-change title (`feat!: `, `fix(scope)!: `, ...) must ship with a
  `docs/releases/unreleased/*.md` fragment — CI fails otherwise. sushiAI is
  pre-1.0: a breaking app change may land in any `0.x` minor; the fragment
  is what actually protects anyone depending on the app, not the version
  number.
- Any user-visible change gets a `docs/releases/unreleased/<slug>.md`
  fragment when its PR is opened. `scripts/release.mjs` folds every pending
  fragment into `docs/releases/<version>.md` at release time and deletes
  them — don't hand-write a dated release note directly.
- Only the owner merges — feature PRs and the release PR alike. This repo
  does not auto-merge anything.
- Use the `ship-pr` skill to open a normal PR, and `cut-release` to prepare
  a release PR — both are procedure around the CI/`scripts/release.mjs`
  machinery described above, not a replacement for it.

## Execution gotchas

- `npm run dev` is not a build. `npm run package` **overwrites
  `release/mac-arm64` with no backup** — confirm before running it.
- `webUtils.getPathForFile()` returns empty paths for drag-and-drop `File`
  objects on Electron 30-33 (`electron/preload.cjs` is the only place it's
  used) — this is a known upstream Electron regression, not an app bug; don't
  re-diagnose it as one.
- Wherever visual drag state is tracked, `onDragLeave` needs
  `!event.currentTarget.contains(event.relatedTarget)` before clearing it, or
  the highlight flickers on every child-boundary crossing
  (`src/ChatView.tsx`, `src/WorkspacePanels.tsx` are the canonical examples).
  `src/agents/AgentsView.tsx` deliberately skips this because it filters on
  `dataTransfer.types` instead and never tracks visual drag state — also
  valid, just a different strategy.
- Tests are `node --test` `.cjs` files importing `.ts` sources directly via
  Node's type-stripping (Node ≥22.18) — no type-checking happens at test
  time, so test assertions have to catch real behavior, not just satisfy the
  type checker.
- `tsc` only type-checks `src/` (`tsconfig.json` `include`) — `electron/` and
  `tests/` get no static typing at all; ESLint covers those directories
  separately.

## Authority and safety

- Never kill another process or free up a port by force without asking whose
  process it is.
- Extensions/plugins render inside the app's own window — never take over
  chrome (the window frame, the left navigation, etc.).
- Shipped source and commit messages are English only, with no AI-attribution
  trailers (already enforced by CI — this is documentation of that policy,
  not a new one).

## Read when relevant

- `docs/AGENTS-INTEGRATION.md` — documents the in-app "Agents" **feature**
  (the product surface for running agents inside sushiAI). Unrelated to this
  file; don't confuse the two.
- `docs/INSTALL.md` — local setup.
- `docs/releases/` — per-release notes.
- `docs/LESSONS.md` — accumulated gotchas from past sessions, kept as a
  capped queue, not an archive.
- `.agents/skills/extension-manifest-change/SKILL.md` — the edit-validator /
  sync-types / update-tests loop for manifest contract changes.
- `.agents/skills/author-and-verify-extension/SKILL.md` — scaffold, validate,
  and smoke-test a new extension end to end. Frontmatter in these SKILL.md
  files beyond `name`/`description` is Claude-specific and silently ignored
  by Codex reading the same file.
- `.agents/skills/ship-pr/SKILL.md` — branch, fragment, verify, and open a
  PR with the right branch/title pairing.
- `.agents/skills/cut-release/SKILL.md` — run `scripts/release.mjs` and open
  the release PR.

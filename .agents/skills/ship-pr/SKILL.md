---
name: ship-pr
description: Opens a sushiAI pull request the way the release pipeline expects: <type>/<slug> branch paired with a conventional-commit title, a docs/releases/unreleased/ fragment when user-visible, npm run ci green first. Use whenever a change is ready to leave the working branch ("ship this", "open a PR", "push this up"). Never merges.
disable-model-invocation: true
---

# Ship a PR

sushiAI merges are squash-only: the PR title becomes the literal commit
message on `main`, and `scripts/release.mjs` reads that history to compute
the next version. A wrong branch prefix or title breaks that pipeline
silently, so this is a checklist, not a suggestion.

## Steps

1. **Branch from fresh `main`**, named `<type>/<slug>`:
   - `feature/<slug>` for a `feat: ` change
   - `fix/<slug>` for a `fix: ` change
   - `chore/<slug>` for anything else (docs, refactor, test, ci, perf, chore)
   - (`release/<slug>` is reserved for `scripts/release.mjs` - never use it
     for a normal change.)

2. **If the change is user-visible**, add a fragment at
   `docs/releases/unreleased/<slug>.md` describing it in the same voice as
   past entries under `docs/releases/` - what changed, why it matters, not
   an implementation diary. Skip this for pure internal/dev-only changes.

3. **Run `npm run ci` locally first.** Don't open the PR to find out CI is
   red; that costs a round trip everyone else can see.

4. **Open the PR** with `gh pr create`, using a conventional-commit title
   that agrees with the branch prefix (`feature/` -> `feat: `, `fix/` ->
   `fix: `, `chore/` -> any other type). `scripts/check-conventions.mjs`
   enforces this pairing in CI - a mismatch fails the `conventions` job.
   A breaking change needs a `!` in the title (`feat!: `) AND a
   `docs/releases/unreleased/*.md` fragment, or the same check fails it.
   Use the repo's PR template body (`.github/PULL_REQUEST_TEMPLATE.md`).

5. **Watch CI**: `gh pr checks --watch`. Report pass/fail back with the
   check names, not just "done."

6. **Never merge it.** Merging is the owner's call, always - this skill's
   job ends at "ready to merge," not "merged."

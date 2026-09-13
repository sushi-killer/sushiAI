---
name: cut-release
description: This skill should be used when asked to cut, prepare, or ship a new sushiAI release/version - it runs scripts/release.mjs to compute the version bump from merged history, folds pending docs/releases/unreleased/ fragments into a real release note, and opens the release PR. Use it instead of hand-editing package.json's version.
---

# Cut a release

`scripts/release.mjs` does the actual work; this skill is the procedure
around it. It never merges or pushes a tag - merging the resulting PR is
what `.github/workflows/release.yml` treats as the release trigger.

## Steps

1. From a clean, up-to-date `main`, run `npm run release:prepare` (add
   `patch`, `minor`, or `major` as an argument only to override the computed
   bump - the default reads merged PR titles since the last tag: any `feat`
   commit means minor, else any `fix` means patch, a `!` breaking marker
   maps to minor while the app stays 0.x, and it refuses to run if none of
   those are present since the last tag).
2. It creates `release/<version>`, bumps `package.json` (+ lock file),
   assembles `docs/releases/<version>.md` from every fragment in
   `docs/releases/unreleased/`, deletes those fragments, and commits all of
   it as `chore(release): v<version>` - already checked out on the new
   branch, nothing pushed yet.
3. Read the assembled `docs/releases/<version>.md` before going further -
   the fragments were written by whoever shipped each PR; this is the last
   chance to fix wording or catch something that reads wrong once combined.
4. Push the branch and `gh pr create` with title `chore(release): v<version>`
   and body pointing at the assembled release note.
5. Tell the owner it's ready: merging this PR (squash) is what publishes
   the GitHub Release and, once that job succeeds, the macOS dmg. Never
   merge it yourself.

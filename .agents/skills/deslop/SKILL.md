---
name: deslop
description: "Diff-scoped AI-slop cleanup for sushiAI: strip narration comments, imagined-state guards, type laundering, one-use helpers and style drift from the current diff without changing behaviour. Use right before autoreview on any agent-written change."
---

# Deslop

Clean only the current diff. Preserve behaviour absolutely.

1. Scope to `git diff $(git merge-base main HEAD)` plus untracked files. Never
   run a repo-wide cleanup.
2. Inspect every changed hunk for:
   - comments a maintainer would not write: narration of the edit, syntax
     explanation, prose restating the code (keep comments that explain a
     non-obvious constraint, ordering or platform quirk);
   - `try`/`catch` or null checks guarding states that cannot occur, abnormal
     for the surrounding module;
   - casts that launder types (`as any`, `as unknown as T`);
   - one-use variables or helpers that add no meaning;
   - compatibility shims, aliases or fallbacks with no named contract;
   - naming, imports or formatting that drift from the surrounding file,
     raw hex colours where a token exists, Cyrillic in shipped source.
3. Fix a finding inline only when the cleanup is trivial and behaviour-neutral;
   otherwise note it for the review.
4. Report in one to three sentences: what changed, what is left for review.

Run `$deslop` before `$autoreview`, never instead of it.

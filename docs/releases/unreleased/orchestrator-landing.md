## orchd: a done task lands on its base branch by itself

- A task created with `land: true` (or `variant.land`) no longer stops at a
  commit on its own branch. When it finishes it joins a landing queue per
  repo and base branch, one task at a time: orchd carries its work onto the
  branch's current head, squashes it to one commit with the task's commit
  message, runs the task's `verify` and `finalVerify` on exactly that tree
  and moves the branch. `landedSha` and a decision line record the result.
- If the base branch is checked out (usually the main checkout) and clean,
  it is fast-forwarded there with `git merge --ff-only`; if it is checked out
  nowhere, `git update-ref` moves it. Nothing is ever pushed.
- Conflicts or failing checks after the carry go back to the task as an
  ordinary failed attempt with the conflict markers, so an agent resolves
  them; the task re-enters the queue after its next passing attempt.
- A checkout with uncommitted changes is never touched: the task waits with
  the new status `landing`, retried every 2 minutes and on `task.start`.
- A landing on the repo's default branch (origin/HEAD, else main/master) is
  refused unless `settings.landOnDefault` is true; the task stays done on its
  own branch and says so in a decision line.
- `settings.afterLand: [{repo, run}]` runs commands in the main checkout after
  a landing (for example a rebuild); each exit code is a decision line and a
  failure never un-lands the task.
- The orchestrator panel shows `landing` as a status and the landed commit in
  the task header.

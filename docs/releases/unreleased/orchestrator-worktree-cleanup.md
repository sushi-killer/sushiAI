## orchd: task worktrees live inside the repo and are removed when no longer needed

- New task worktrees are created at `<repo>/.sushiai/worktrees/<slug>`
  (setting `worktreeRoot`; a relative value is resolved against the repo
  root). orchd adds `/.sushiai/` to the repo's `.git/info/exclude`, never to a
  tracked file. Existing tasks keep the worktree path they were created with.
- A task that ends `done` has its worktree removed (its work is committed on
  its branch or landed on its parent), recorded as `worktreeRemoved: true`
  plus a decision line. A parent removes its own worktree when it finishes.
- Archiving a stopped or failed task first saves its uncommitted work (tracked
  and untracked, not ignored) to its wip ref, then removes the worktree.
  Deleting a task removes its worktree and branch. A running, waiting, queued
  or drafting task never loses its worktree.
- `task.start`, a retry, or unarchiving a task whose worktree is gone
  recreates it from its branch and restores the saved work.
- New `orchd gc --data <dir> --socket <sock> [--dry-run]` (RPC
  `worktrees.gc`) applies the same rules to every task of every repo, then
  prunes, and reports each path, the action and the bytes freed. A worktree no
  task knows about is only listed. A dry run changes nothing.
- This repo ignores `.sushiai/` in git, prettier and eslint, so CI never walks
  nested worktrees.

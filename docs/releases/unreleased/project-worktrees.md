## Worktrees

- Project settings has a Worktrees tab (above Hosts). It lists the project's git worktrees on this Mac and on every SSH host the project has a folder on: branch, host, age of the last commit, and status — uncommitted changes, merged into the base branch (squash merges count), commits not yet merged, or a folder that is already gone.
- The trash button removes a worktree: its folder is deleted, sessions open in it stop and leave the sidebar, and its branch goes too when you choose to (on by default only for merged branches). A worktree whose folder was deleted by hand is just forgotten. "Remove N merged…" clears every clean, merged worktree at once.
- The main checkout and the worktree of an orchestrator task that is still running cannot be removed.

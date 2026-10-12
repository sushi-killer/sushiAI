## orchd

- The first implement attempt of a task is briefed with a `## Where to look` block: the planner's paths for the task, each file with the start of its text (at most 40 lines) and each directory with its entry names. Paths outside the worktree, missing and binary files are left out. A conflict-only attempt does not get it.
- A task report now lists, for each implement attempt, its exploration calls and the time to its first edit, and a `Total time` line from creation to done.

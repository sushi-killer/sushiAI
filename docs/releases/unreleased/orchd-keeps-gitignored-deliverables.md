## Task deliverables in gitignored paths survive the worktree

- A file a task's goal, criteria or verify commands name (for example `artifacts/workflow-inventory.md`) that is gitignored or untracked is now copied into the task's data folder before its worktree is removed, whether the task finished, failed or was stopped. After a successful land it is also placed at the same path in the repository's working copy when that path is gitignored there and free; an existing file is never overwritten.
- The task report and the task detail list each deliverable with where it was kept, and the task detail has an Open button for it.

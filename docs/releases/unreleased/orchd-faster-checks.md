orchd shares a dedicated sccache compiler cache across task worktrees for setup,
verification, base checks and final gates, while each worktree keeps its own
build outputs. Gate decisions report cache hits from the server's stats delta.
Set `compilerCache: false` in `.sushiai/setup.json` to opt out. Agents whose
sandbox cannot reach the server or write the cache directory skip the wrapper
and record the reason.

- orchd reuses checks that already passed on the same worktree tree across later gates, avoiding duplicate verification and final checks. Checks that mention `ORCHD_BASE_SHA` run again when the base changes.
- Interrupted checks get one immediate rerun. Timeouts and interrupted exits are never replayed from the base or verify caches, so later retries run the check afresh.

- Subtasks gated at their parent run package and matching Node tests for their changed files in place of broad CI commands. Unsafe or unmapped changes keep the full check; scoped commands remain on the parent's combined-tree gate.
- Repos can declare path-scoped shared checks in committed `.sushiai/setup.json`. The declarations merge with the settings policy and are read at the task's base commit, so a task cannot edit them to skip its own checks.
- Desktop smoke reuses the build stamped by CI when tracked and untracked inputs are unchanged, and rebuilds after edits. Tree snapshots rehash tracked content without using the real Git index's cached file stats, so same-size rapid edits invalidate both build stamps and passed-check reuse.

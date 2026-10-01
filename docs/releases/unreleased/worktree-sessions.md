# Worktree sessions

- New worktrees start from `main` by default. Pick another base branch in the panel dialog, including branches available only on `origin`. Before creation, the selected branch's upstream is fetched on its host; `main` uses `origin/main` when no upstream is configured. A failed fetch stops creation. Repositories without a remote use their local base branch.
- The launch host picker lists each machine once instead of listing its worktree branches as hosts.
- Closing the last session removes its empty worktree workspace without an extra dialog, including local sessions and already-ended panes. Closing a primary checkout's last pane preserves its linked worktree sessions instead of failing with a worktree-group error. Checkout folders and Git branches remain available.

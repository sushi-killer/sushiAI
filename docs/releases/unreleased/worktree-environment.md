## Orchestrator

- Task worktrees now live outside the repo, under `~/.sushiai/worktrees.noindex/<repo>/`. The `.noindex` suffix keeps Spotlight from indexing worktrees, `node_modules` and build output. The old `.sushiai/worktrees` setting counts as the new default; any other folder you set still works. Tasks that already exist keep their path.
- A repo can commit `.sushiai/setup.json` to prepare each new worktree (install dependencies). It runs once per worktree, also for base checks. A failing setup fails the task with the output tail and no attempt. Without it, a repo with a lockfile and no `node_modules` gets the matching install.
- `.worktreeinclude` lists ignored files to copy into each worktree. Without it, `.env`, `.env.local` and `.claude/settings.local.json` are copied as before.
- orchd no longer links `node_modules` from the main checkout into a worktree.
- Setup, verify and agent processes use `RUSTC_WRAPPER=sccache` when sccache is installed and the variable is not set.

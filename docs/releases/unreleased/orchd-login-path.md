## Orchestrator

- Tasks and their checks now see the tools your shell sees: orchd adds your login shell's PATH (and `~/.cargo/bin`) to every command it runs, so a check like `npm run ci` that calls `cargo` no longer fails with "command not found" when sushiAI is started from the Dock or orchd from ssh.
- A check that fails on the base because a program is missing (exit 127) keeps gating and says so in the task's decisions, instead of being switched off as unrelated to the task.

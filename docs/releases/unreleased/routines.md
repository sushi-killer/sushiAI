## Routines

- Routines are now scheduled background agent runs. Describe one in a sentence; a builder session (Claude Code or Codex) picks the project and host, writes the brief, checks it, saves a draft and runs a dry run that writes nothing.
- Each routine runs on its host's daemon by cron (at most every 5 minutes), with its own agent, model and thinking level. A run ends with one validated JSON result; `noop` runs are archived quietly, `needs_you` and `failed` reach the Inbox.
- The last 30 runs are kept, and three failed runs in a row pause the routine.
- Codex routine runs use workspace-write without network access and with an empty MCP server list; their asks go through a file channel the daemon serves, so a run never reaches the daemon socket. MCP tools cannot be granted to a Codex routine yet.
- The old saved shell-command routines are removed with no migration. The extension API is unchanged.
- A routine lists the tools it needs as `access`. You grant each one; a run may use only the granted tools, and a routine with ungranted access ends `needs_you` without counting a failure.
- Run modes are real: ask every time (a proposal waits for your answer), ask first (a proposal waits too, until its kind reached `promoteAfter` accepted asks) and on its own. A routine whose files change outside the app is refused and paused until you save it again.
- The builder can reach the CLI tools through an MCP server (`sushiai mcp --routines`), and Settings choose what it sees and whether it uses the commands, MCP or both.
- A run that failed on a login records the host and harness, so the app can open a terminal that runs the login there.
- A routine can work on many projects (zero to 20). With one project it keeps its own git worktree; with two or more it reads each project's folder and is read-only for now (Claude only; `sushiai propose` is refused). A routine saved with one `project` loads as a list of one.
- A dry run now uses the access you granted when it is read-only, so a routine that reads through connector tools can be dry-run. Write tools and ungranted ones stay unavailable and the result names them.
- A routine can list agent skills (`skills` in `routine.json`) and use them with a short brief. A Claude routine without skills cannot call the Skill tool. A skill's scripts need an access grant in a narrow form (`Bash(python3 /path/skills/<name>/...:*)`). The builder finds skills with `sushiai skills`.

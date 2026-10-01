## Project environments

- Configure project variables, secrets, MCP servers, Claude accounts, and setup commands once, then use the project on local and trusted SSH hosts.
- Remote project checkouts use `~/sushiai/<slug>` by default, and the new project flow can prepare local or remote checkouts.
- Creating or preparing a project pulls an existing checkout with a fast-forward only update, and skips it without losing work when the tree has local changes or has diverged.
- A project reads `.env`, `.env.local`, `.mcp.json` and the Claude MCP servers of its folder automatically; secret-looking values are stored as secrets and MCP configs keep only `${VAR}` references.
- Preparing a host shows each step (clone, install, check), asks before sending secrets, and reports which step failed.

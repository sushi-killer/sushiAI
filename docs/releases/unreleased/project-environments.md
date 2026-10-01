## Project environments

- Configure project variables, secrets, MCP servers, Claude accounts, and setup commands once, then use the project on this Mac and on every SSH host you added; a "Don't send secrets to this host" switch in Project settings → Hosts keeps a project's values off a host.
- Remote project checkouts use `~/sushiai/<slug>` by default, and the new project flow can prepare local or remote checkouts.
- Creating or preparing a project pulls an existing checkout with a fast-forward only update, and skips it without losing work when the tree has local changes or has diverged.
- A project reads `.env`, `.env.local`, `.mcp.json` and the Claude MCP servers of its folder automatically; secret-looking values are stored as secrets and MCP configs keep only `${VAR}` references.
- The "+" picker can start a session on a host that is not set up yet: choose its chip, then "Prepare <host> and start" clones, installs and starts there with no extra question. A failed clone shows the steps and what to do (try again, use the host's git login, edit the token). Its title lists your open projects to switch between.
- Project settings opens for every workspace: a folder with a git remote (whatever it is called) joins that remote's project, a folder without one gets a project of its own and follows a remote added later. Environment and MCP pull what the folder holds on open and say what they added; "Import from folder" pulls again, and the name is edited in General.
- The New project flow clones, installs and sends the values to each chosen host, then goes to Ready; there is no approval step.
- The MCP servers tab counts real uses over 30 days for your Claude config servers and plugins.
- Preparing a host shows each step (clone, install, check) and reports which step failed and what reached the host.

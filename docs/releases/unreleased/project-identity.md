## Projects

- One repository is one project in the sidebar, whatever its folder is called and on whichever host or worktree it is checked out. The flat list shows it as one row; grouping by host still splits it per host.
- sushiAI remembers each folder's git identity and project in a local database (`sushiai.db`, imported once from `projects.json`, which is kept as `projects.json.imported`). A checkout on an SSH host that is offline still joins its project, and git is read once per start instead of on every refresh.
- A checkout on a host sushiAI has never reached still shows as its own row until the host answers once.

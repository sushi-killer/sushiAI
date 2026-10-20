## One agent catalog

- `sushiai agents catalog` prints what each agent CLI on this host offers: its
  models, the reasoning levels of each, the levels the routine builder offers and
  the permission modes. `--json` prints the same for a script.
- The daemon serves the same table as `agents.catalog` (capability
  `agentsCatalog`), so the app, the routines page and the link read one list
  instead of their own copies.
- Codex's model list and its default model and effort come from the CLI's own
  cache and `config.toml` on that host; a host without them still reports its
  levels.

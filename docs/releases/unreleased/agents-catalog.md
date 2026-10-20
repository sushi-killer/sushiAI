## One agent catalog

- The daemon serves what each agent CLI on a host offers as `agents.catalog`
  (capability `agentsCatalog`): its models, the reasoning levels of each, the
  levels the routine builder offers and the permission modes. The app and the
  routines page read it instead of their own lists.
- Codex's model list and its default model and effort come from the CLI's own
  cache and `config.toml` on that host; a host without them still reports its
  levels.
- The routines page takes its models, levels and model notes from that catalog.
  An ssh host's Codex models come from the daemon on that host, not from a file
  read over ssh.
- A sushiAI daemon older than this build (a local one kept running across an
  update, or one on an ssh host) shows empty model, level and mode pickers until
  it is restarted or updated. The app flags it as needing an update. Chats still
  run: the agent CLI itself judges the model, level and mode that were picked.

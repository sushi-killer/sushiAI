## Full access for Claude Code and Codex

- The "+" panel picker shows a "Full access (skip permission prompts)" checkbox
  under Claude Code and under Codex. Ticked, the agent starts with no permission
  prompts and no sandbox: Claude Code with `--permission-mode bypassPermissions`,
  Codex with `--dangerously-bypass-approvals-and-sandbox`. Agents without such
  a mode show no checkbox.
- The picker remembers the last choice for each agent. It starts off.
- A pane started this way carries a "Full access" tag in its header, taken from
  what the daemon reports. A session that wakes or is reopened keeps the mode.
- A sushiAI daemon older than this build ignores the choice and starts the agent
  guarded, without the tag. Restart or update it to use the checkbox.
- A workspace created on a host that is not set up yet, and the new-workspace
  dialog, still start agents with prompts on.

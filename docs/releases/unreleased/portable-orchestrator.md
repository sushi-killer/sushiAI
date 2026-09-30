## Orchestrator

- The orchestrator's prompts (chat, Brainstorm, Plan and the MCP server's instructions) now live in `orchd/prompts/orchestrator.yaml`. Put a `prompts.yaml` with any of those keys in the orchestrator's data folder to override them; the change applies on the next turn.
- sushiAI installs a `sushiai-orchestrator` skill into Claude Code and Codex (on this Mac and on remote hosts it starts orchd on), so an agent in any project knows how to create, plan, route and answer orchestrator tasks, including the exact command to attach the orchestrator's MCP server.

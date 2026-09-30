## Orchestrator

- Tasks that run on Codex now get the MCP servers attached to the task itself (for example a design tool), not only orchd's messages server. Connected tools and the repository's own `.mcp.json` stay Claude-only, because Codex has no hook that asks you before a tool writes.

## Orchestrator

- A task that runs on Codex can now use an MCP server attached to the task (for example a design tool): mark it `"codex": true` in the task's `mcp`. Only marked servers reach Codex, never the orchestrator's own bridge, because Codex has no hook that asks you before a tool writes; Claude runs keep getting every task, connected and repository server with writes gated as before.

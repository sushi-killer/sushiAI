## The orchestrator chat keeps each reply's tool calls

- Each finished orchestrator reply now carries the tools it called while answering, as a `tools` list of `{name, summary}` entries, so the app can show a muted tool line above the reply.
- The name drops the orchestrator server prefix (`task_get`, not `mcp__sushiai-orchestrator__task_get`); the summary is the task's title, the title of a task being created, or the path or pattern of a Read, Grep or Glob call. Task ids never appear in it.
- Repeated back-to-back calls count once, calls made by subagents are skipped, and at most the first 8 calls of a reply are kept.
- The orchestrator and brainstorm prompts now tell the agent to refer to tasks by their title, never by an id, a bare id prefix or a run id, in text meant for the owner.
- The protocol change is additive: chats saved before this change load unchanged, and a message with no tool calls looks exactly as before.

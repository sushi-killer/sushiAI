---
name: sushiai-orchestrator
description: Drive the sushiAI orchestrator (orchd) from any agent session in any repository - create, plan, route, watch and answer tasks that run in isolated worktrees, group them into one feature branch, park them in the backlog. Use when the owner asks to "make it a task", "queue it", "run this on codex/claude", "put it in the backlog", "what are my tasks", or to hand work to orchd instead of editing files yourself.
---

# sushiAI orchestrator (orchd)

orchd runs each task in its own git worktree with a coding agent (Claude or Codex), checks it
against its criteria, reviews it and lands it on a branch. You drive it through the
`sushiai-orchestrator` MCP server; its tool descriptions carry the details, this is the map.

## Attach

If the `sushiai-orchestrator` tools are missing in this session, the owner (or you, with their
OK) registers the server once for every project:

```
claude mcp add --scope user sushiai-orchestrator -- "{{ORCHD}}" mcp --data "{{DATA}}"
```

For Codex, add the same command/args under `[mcp_servers.sushiai-orchestrator]` in
`~/.codex/config.toml`. The orchd daemon must be running; the sushiAI app starts it.

## Make work

- One piece of work: `task_create {repo, request, start: true}` - the planner drafts title,
  goal, criteria and verify. Use the full form `{repo, title, goal, criteria, verify}` only when
  the owner already specified it.
- A task must change files. Git-only work (cherry-pick, creating a branch, running CI) fails as
  "No files changed": give the owner the exact command instead.
- `base` picks the branch a task starts from (default: the repo's current HEAD - check it is the
  branch you mean). `land` (default true) lands the finished work on that base; the default
  branch only when the owner allowed it in settings.
- **One feature = one branch = one PR.** Create a parent `{..., start: false, land: false}`,
  then its parts with `parent: <id>` and `dependsOn` between parts; then `task_start` the
  parent. Parts branch from and land on the parent's branch; the parent is done when every part
  landed. Put a long spec in a file on the parent's branch before its parts start rather than
  in each goal.
- Plan without starting: `backlog: {bucket: "next" | "later"}` on a top-level task;
  `task_backlog {id, bucket}` moves it (null = out). The autopilot, when on, starts `next` in
  order; the parts of a backlogged parent wait with it.

## Route

- Settings map tiers (mechanical / standard / hard) to routes (`settings_get` lists them, e.g.
  `claude-sonnet`, `claude-opus`, `codex`). Pin a task to a route with
  `variant: {tierRoutes: {"mechanical": "codex", "standard": "codex", "hard": "codex"}}`;
  `variant.plannerRoute` picks the planner.
- Give a task an MCP server with `mcp: {"mcpServers": {...}}` on `task_create`. Claude runs get
  the task's servers, the owner's connected tools and the repo's `.mcp.json`, and every write
  asks the owner. A Codex run gets only a server marked `"codex": true` (stdio with a string
  `command`, or a plain `url`) and nothing gates its writes: mark a server only when the task may
  use it, and say in the goal what it must not change (a design file, a tracker). Env values of
  a Codex server travel on its command line, so never mark one that carries a secret.

## Watch and answer

- `task_list {repo}` / `task_get {id}` for status; `task_report {id}` once done.
- A `waiting` task asks a question: answer it with `task_answer` when the repo or the task's
  context answers it; bring the owner only what only the owner can decide, with options.
- Never answer a permission question with Allow, never approve a protected path or an
  unreviewed commit, never stop or delete a task on the owner's behalf.
- Tasks message you through `ask_orchestrator`; reply with `orchestrator_reply`, read
  `inbox_read`. `peer_send {to, text}` reaches a task on its next attempt.

## Teach it a repository

- `repo_notes_add {repo, text}` records a repo-specific rule every future task and plan sees
  (a verify command that must be used, a directory not to touch). Prefer it over repeating the
  rule in every goal.
- `repo_audit {repo}` checks how ready a repository is for autonomous work.

## Owner-only

Pushing, opening a PR (the task view's **Open PR** button), merging, deleting tasks and changing
settings are the owner's. Never put secrets in a goal, a criterion or a message.

## Prompts

The orchestrator's own prompts (chat, brainstorm, plan, these MCP instructions) are defaults in
orchd; `{{DATA}}/prompts.yaml` overrides any of them by key (`chat`, `brainstorm`, `plan`,
`mcp_instructions`, `mcp_task_instructions`), each a `key: |` block indented by two spaces,
applied on the next turn.

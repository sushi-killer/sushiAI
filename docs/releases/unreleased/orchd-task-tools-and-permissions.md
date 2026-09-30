## orchd: a task gets the tools its criteria need, and a missing permission is one owner question

- A task run now starts the same connected tools the orchestrator chat has
  (the ones enabled in Settings and already seen reachable), the repository's
  own `.mcp.json` servers and the task's own `mcp`. Only read tools are
  pre-allowed. The brief and the planner's brief list the tools the task will
  have.
- A call the run may not make no longer fails silently: a write tool of an
  outside service (Slack, Asana, Sheets, ...), `WebFetch`/`WebSearch` and a
  write under `.claude/**` hold the call and raise one owner question,
  "Allow <tool or path> for this task?", with Allow once / Always for this
  repo / Deny. The attempt goes on with the answer, no retry. "Always" is a
  per-repo rule later tasks inherit; a write to an outside service asks on
  every call unless it was "Always".
- Claude Code refuses `.claude/**` to agents even when a hook allows it, so an
  allowed write goes to `.orchd-staging/` in the worktree and orchd copies it
  to the real path before verify.
- The brief check also asks whether a criterion needs a capability the task
  will not have (an Asana move with no Asana tool): before the first attempt
  the owner may enable the tool for the task, make the criterion an owner
  action, or continue.
- A refusal the run reports at its end, or a `No tool: <capability>` blocked
  report, stops after that first attempt and asks the owner instead of
  starting another attempt that would fail the same way; the same refusal
  again after the owner answered ends the task.

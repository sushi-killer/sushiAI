## Orchestrator: a large request becomes subtasks that land on one branch

- When a request is too large for one agent session, the planner can split
  it into 2-5 subtasks, serial where one builds on another. The task becomes
  their parent: it runs no attempt of its own, and each subtask is planned
  and run as a normal task on a branch taken from the parent's.
- Subtasks declare the paths they touch; siblings whose paths overlap (or
  that declare none) run one after another, with a decision line on the
  parent for each added ordering.
- Independent subtasks run side by side. A dependent one starts only after
  the subtask it builds on is done, from a branch that already contains it.
- A finished subtask lands on the parent's branch through a queue, one at a
  time: its work is moved onto the branch as it is now, checked again, and
  added on top. A conflict or failing check goes back to its agent to fix,
  never silently dropped.
- The parent is done when every subtask has landed and its own checks pass
  on its branch; its cost includes its subtasks'. You merge the parent's
  branch as usual.
- If a subtask or a dependency ends failed or stopped, the tasks waiting for
  it ask you: retry it, drop it, or stop.
- The orchestrator agent can build such a graph by hand: `task_create`
  accepts `parent` and `dependsOn` (task ids), and rejects an unknown id or
  a dependency cycle without creating anything. A task that is already
  running cannot be made a parent, so it never implements the request
  itself.
- A parent's detail in the Orchestrator panel lists its subtasks with their
  statuses.
- Tasks without a parent or dependencies, and plans without subtasks, work
  exactly as before.

## orchd: finished work lands by default, with a Land button

- `land` is on by default for top-level tasks (`settings.experiments.land`
  and `task.create`). A task lands on its non-default base branch when it is
  done; the default branch is still refused unless `settings.landOnDefault`,
  and `land: false` on a task still opts out.
- New RPC `task.land {id}` lands a done, unlanded top-level task now through
  the existing landing queue. The orchestrator panel shows a Land button in
  a done task's header, and the done notice offers a Land action.
- A done row now says `landed` or `not landed` next to the verdict and cost.
- Tasks record a `source` (chat, ui, mcp, cli, planner, eval, handoff) at
  creation, and `costs.summary` counts tasks and cost by source in
  `tasksBySource`.
- A rebuilt orchd binary (for example the `afterLand` rebuild after a
  landing) no longer restarts the daemon under running attempts: the app
  waits until none run, for at most 30 minutes, so landings stop cutting
  every live task short.

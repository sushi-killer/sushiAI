## orchd: plan backlog and autopilot

- A task can sit in a planning backlog: `task.create` takes an optional
  `backlog: {bucket: "next" | "later", order?}` and the new `task.backlog
  {id, bucket, order?}` moves a task between buckets or clears it with
  `bucket: null`. An omitted order appends to the end of the bucket. Task
  JSON carries a top-level `backlog` while it is set. Only a top-level task
  that has not implemented anything and has no live loop can be parked.
- A backlog task is never started by the graph or by daemon recovery, even
  with `dependsOn`; `task.start` (decision `Owner: started from the backlog`)
  or the autopilot starts it and clears the backlog.
- `settings.autopilot` (off by default) starts ready `next` tasks, lowest
  order first, whenever fewer than `parallel` task loops are live: after a
  loop finishes, on daemon start, and after `task.backlog`, `task.create`
  and `settings.set` (decision `Autopilot: started from the next backlog`).

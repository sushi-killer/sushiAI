## Fix: orchestrator chat no longer leaks its process on shutdown

- Stopping `orchd` (the `shutdown` RPC or SIGTERM) while the orchestrator
  chat was mid-reply used to leave the `claude`/`codex` child running
  forever, orphaned, with nothing left able to cancel it. The daemon's
  shutdown path now cancels a live chat turn the same way it already
  cancelled a running task attempt, and waits for it to actually exit
  before the process does.

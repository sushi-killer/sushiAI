# Orchestrator check concurrency

orchd limits check commands per machine separately from agent turns, so parallel tasks leave capacity for each other’s checks. The Advanced JSON setting `heavyChecks` defaults to `null`: max(2, logical cores / 5), or 3 on a 15-core machine. Set a number to choose the limit, or `0` for no limit. A task waiting for a check slot records its wait in the decision list.

Both `parallel` and `heavyChecks` now take effect without restarting orchd. Raising a limit wakes waiting work; lowering it lets running work finish and caps new starts.

Check results record execution time, the 1-minute load average and logical core count at their start. A timeout starting at a load at least the core count reruns once as an environment failure, taking a check slot again. A failing rerun is an ordinary verification failure and never asks about a pre-existing base failure. Slot waits do not count toward execution time or the check timeout.

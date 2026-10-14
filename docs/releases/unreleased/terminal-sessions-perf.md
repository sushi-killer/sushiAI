## Terminal sessions stay responsive under heavy output
- The daemon batches terminal output per tick and the renderer writes it once per frame, so a session that prints a lot no longer floods IPC.
- On the benchmark, remote input p95 drops from 746 ms to 44 ms, frame p95 from 50 ms to 16.7 ms, and IPC messages from 3158 to 412.
- `node scripts/terminal-perf.mjs --compare-base` measures the change against the base branch; see `docs/perf/terminal-sessions.md`.

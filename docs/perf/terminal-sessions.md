# Terminal session performance

`npm run perf:terminal` measures a terminal session end to end: a flood of
output through `createTerminalHandlers`, the same flood into the real
`TerminalPanel` in headless Chromium, and keystroke-to-echo latency through a
real daemon. stdout carries one JSON object; `PERF_OUTPUT=<file>` also writes it.

## Run

```
cargo build -p sushiai
PERF_OUTPUT=$TMPDIR/before.json npm run perf:terminal
npm run perf:terminal -- --quick --only=output,input
npm run perf:terminal -- --compare $TMPDIR/before.json $TMPDIR/after.json
ORCHD_BASE_SHA=<base commit> node scripts/terminal-perf.mjs --compare-base
```

No result JSON is committed: before and after come from one
`--compare-base` run on the machine that runs it.

- `--quick`: about 20k lines and 20 keys.
- `--only=output,input,renderer`: a comma list, default all.
- `--compare <before.json> <after.json>`: a markdown table of every numeric
  metric with before, after and delta (absolute and %).
- `--compare-base`: runs the benchmark twice with the same config, first in a
  temporary git worktree at `$ORCHD_BASE_SHA` (it borrows this tree's
  `node_modules` and `target`, and is removed afterwards), then on this tree.
  The base may predate the benchmark, so this tree's harness
  (`scripts/terminal-perf.mjs`, `scripts/lib/perf-compare-base.mjs`,
  `tests/terminal-perf-harness.tsx`) is copied into the worktree and measures
  the base's `electron/`, `src/` and daemon. It prints the `--compare` table
  and a checklist, and exits 1 unless the renderer's blocked time or frame p95
  is at most 0.85x before, the remote keystroke p95 is at most 0.5x before and
  within local p95 + 1.5 RTT, and `bufferLines` is at most 10200. `--quick`
  and `--only` pass through to both runs. The input section starts a real
  daemon on a unix socket, so it needs a machine (not the agent sandbox) that
  may bind one.
- Env: `PERF_LINES`, `PERF_FRAME_BYTES`, `PERF_RTT_MS` (default 40),
  `PERF_KEYS`, `PERF_KEY_INTERVAL_MS`. `TERMINAL_TEST_BROWSER` picks the
  Chromium executable. Without it the renderer uses Playwright's own build if
  it is installed, else the newest
  `chromium_headless_shell-*/chrome-headless-shell-linux64/chrome-headless-shell`
  or `chromium-*/chrome-linux64/chrome` under `PLAYWRIGHT_BROWSERS_PATH` or
  `~/.cache/ms-playwright`. The chosen path is logged to stderr.

## Config

`config` in the JSON: `lines` (200000, 20000 quick), `frameBytes` (4096),
`rttMs` (40), `keys` (60, 20 quick), `keyIntervalMs` (30), `quick`.

## Metrics

`output` (Node only): `lines`, `bytes` (flood size), `ipcMessages` (sends to the
renderer), `sentBytes`, `wallMs`, `eventLoopMaxMs` / `eventLoopP99Ms`
(`monitorEventLoopDelay`), `snapshots`. The fake `send` acks on the next turn.

`renderer`: `drainMs` (first frame to the final marker visible in
`.xterm-rows`), `frameP50Ms` / `frameP95Ms` / `frameMaxMs` (rAF intervals during
the flood), `blockedMs` (long-task time over 50 ms), `longTaskMaxMs`,
`heapUsedMB` (after a forced GC), `domNodes`, `bufferLines`, `ipcMessages`,
`snapshots`. An overflow re-attach is answered with the last 2000 lines.

`input`: `local` (direct socket) and `remote` (a proxy delaying each chunk
rttMs/2 each way), each with `p50Ms`, `p95Ms`, `maxMs`: the time the i-th
echoed printable character leaves `send` minus the time key i was typed, with
`/bin/cat` in the session, through `createTerminalInput` -> `handlers.write`.

## What the numbers point at

- Per-frame IPC: every 4 KB daemon frame becomes at least one message
  (`ipcMessages`), each paid in structured clone and an ack back.
- The per-character loop in `flush` (terminals.cjs): `Buffer.byteLength` per
  code point of the queue head, visible in `eventLoopMaxMs` / `wallMs`.
- Overflow resyncs: a renderer that acks slower than the flood passes
  `MAX_QUEUED_BYTES`, drops its queue and re-attaches; `snapshots` above 1 and
  the lost scrollback show it.
- The per-keystroke round trip: `createTerminalInput` awaits each `write`
  before the next, so `remote` p50 is bounded below by `rttMs` and grows once
  keys come faster than one round trip (`keyIntervalMs` < `rttMs`).

## Machine and baseline

Linux 6.8.0-142-generic, 11th Gen Intel(R) Core(TM) i7-11800H @ 2.30GHz (16
threads), Node v22.23.2.

The baseline (200k lines, unchanged product code) was recorded with the
`output` and `renderer` sections only: the `input` section needs the real
daemon to bind a unix socket, which the agent sandbox forbids. The full
before/after/delta, input included, comes from

```
cargo build -p sushiai
ORCHD_BASE_SHA=<base commit> node scripts/terminal-perf.mjs --compare-base
```

on a machine that allows it; paste the table it prints into the PR body.

The renderer run shows the causes: 3158 output IPC messages for 12.9 MB, 11
snapshots (overflow resyncs) leaving only 10027 buffer lines, and a frame p95 of
about 50 ms.

## After: batching per tick and per frame

Before and after come from `--compare-base`, which runs the unchanged
benchmark on the base commit and on this branch on the same machine. The
sandbox that made this change cannot bind the daemon's unix socket (EPERM), so
the table below has no `input` section; orchd runs the full
`--compare-base` outside it. The keystroke path is also covered by
`tests/daemon-terminals-batching.test.cjs` (every key echoes in under 150 ms
over a 40 ms round trip).

What changed:

- Main (`daemon/terminals.cjs`): output that arrives in one event-loop turn is
  flushed once (`setImmediate`), merged into messages of at most
  `OUTPUT_CHUNK_BYTES`, each measured with one `encodeInto` that stops on a
  whole character. An exit waits until the output queued before it has gone
  out (or was replaced by a resync snapshot), even while the credit is out; a
  host that goes away sends its queue in full before the exit.
- Renderer (`createOutputBatcher` in `terminal-output.ts`): what arrives within
  one frame (rAF, or a 50 ms timeout in a hidden window) is written to xterm in
  one write and acked once parsed. A snapshot drops the output still pending
  and acks its bytes.
- Input (`createTerminalInput`): each keystroke goes out at once instead of
  after the previous write's round trip; the 1 MB / 1024-write cap counts what
  is in flight.
- The next causes, found after the first two:
  - With main no longer slowed by its per-character loop the flood reached the
    renderer faster and the overflow resyncs came as a storm (each re-attach
    overflowed again before the renderer had acked the window before its
    snapshot), so frames got worse (p95 83 ms). An entry whose renderer still
    owes acks for the window before its last snapshot now drops output on
    overflow and re-attaches once that window is acked. A renderer that stops
    acking still gets its first resync at once.
  - xterm parses for up to 12 ms per task and yields only between writes. A
    2000-line resync snapshot (about 125 KB) was one write, about 12 ms of
    parsing on its own, so about one run in three still had a 33 ms frame. The
    batcher now writes in slices of at most 32 KB (a snapshot cut after a line
    break) and hands xterm at most 64 KB per frame; the rest waits, unacked,
    for the next frame. Over 40 runs of the renderer section, 2 had one 33 ms
    frame; the others stayed at 16.7 ms.

`bufferLines` is lower because the flood now ends on the last resync snapshot
(2000 lines of history) instead of a resync storm followed by more of the
flood.

`ORCHD_BASE_SHA=5886e961 node scripts/terminal-perf.mjs --compare-base --only=output,renderer`:

| metric                 |   before |    after |   delta | delta % |
| ---------------------- | -------: | -------: | ------: | ------: |
| output.lines           |   200000 |   200000 |       0 |      0% |
| output.bytes           | 12928598 | 12928598 |       0 |      0% |
| output.ipcMessages     |     3158 |      412 |   -2746 |    -87% |
| output.sentBytes       | 12928598 | 12928598 |       0 |      0% |
| output.wallMs          |   312.95 |    72.06 | -240.89 |    -77% |
| output.eventLoopMaxMs  |     3.86 |     3.63 |   -0.23 |     -6% |
| output.eventLoopP99Ms  |     1.95 |     1.29 |   -0.66 |  -33.8% |
| output.snapshots       |        1 |        1 |       0 |      0% |
| renderer.drainMs       |   487.97 |    124.4 | -363.57 |  -74.5% |
| renderer.frameP50Ms    |     16.7 |     16.7 |       0 |      0% |
| renderer.frameP95Ms    |     33.4 |     16.7 |   -16.7 |    -50% |
| renderer.frameMaxMs    |     33.4 |     16.7 |   -16.7 |    -50% |
| renderer.blockedMs     |        0 |        0 |       0 |       - |
| renderer.longTaskMaxMs |        0 |        0 |       0 |       - |
| renderer.heapUsedMB    |     9.62 |     7.93 |   -1.69 |  -17.6% |
| renderer.domNodes      |      127 |      127 |       0 |      0% |
| renderer.bufferLines   |    10027 |     2001 |   -8026 |    -80% |
| renderer.ipcMessages   |      468 |       22 |    -446 |  -95.3% |
| renderer.snapshots     |       11 |        4 |      -7 |  -63.6% |

## Not fixed here

- ssh runs `sushiai proxy` without a tty, so OpenSSH does not set
  `TCP_NODELAY` on that connection and Nagle can hold a small keystroke packet
  behind an unacked one. Forcing `-t` would put a pty into the binary framing,
  and no benchmark here reaches a real ssh hop, so this is a finding only.

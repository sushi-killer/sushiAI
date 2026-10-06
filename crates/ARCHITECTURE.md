# Crates

A Cargo workspace for the session daemon. It is not wired into the app yet. The
desktop app still uses Herdr and orchd; see `docs/architecture.md`.

## Shape

```
sushiai ──► sushiai-daemon ──► sushiai-core ──► sushiai-protocol
   └──────► sushiai-hold ──────────────────────► sushiai-protocol
```

- `sushiai-protocol`: wire types and the frame codec. No IO, no tokio.
- `sushiai-core`: pure session logic. Screen model over `vt100`, session
  transitions, state file model. No IO, no tokio.
- `sushiai-daemon`: tokio shell. Socket server, one actor per session, holder
  client, state file IO, holder spawning.
- `sushiai-hold`: synchronous holder. PTY, in-memory output ring, unix socket.
  Threads, no async runtime.
- `sushiai`: the binary. Parses arguments and calls the libraries.
  Subcommands: `daemon`, `status`, `hold` (internal, started by the daemon).

`scripts/check-crate-deps.mjs` enforces the arrows above in CI.

## Where things are

- `sushiai-protocol/src/frame.rs`: frame codec. `vectors/*.json` are golden
  frames; a TS client will read the same files.
- `sushiai-protocol/src/methods.rs`: method names, error codes, params and
  results.
- `sushiai-daemon/src/session.rs`: the session actor.
- `sushiai-daemon/src/server.rs`: client connections and method dispatch.
- `sushiai-daemon/src/registry.rs`: session catalog and `state.json`.
- `sushiai-hold/src/lib.rs`: holder loop. `ring.rs`: output ring.
- `sushiai/tests/daemon.rs`: end-to-end test with real processes.

## Invariants

- A frame is `u32` BE length of (kind + payload), a kind byte (`J` JSON-RPC,
  `B` output), then the payload. Maximum 16 MiB. The decoder never reads IO.
- `seq` is the byte offset of a chunk in the session's whole output stream.
  The holder ring and the daemon screen agree on it.
- The holder is its own session (`setsid`). Killing the daemon, even with
  SIGKILL, never kills a holder or its child.
- One client at a time on a holder. A new connection replaces the old one and
  replays the ring from the start.
- A session actor owns its screen, its `seq` and its holder connection. Other
  tasks reach it only through its bounded command queue and output broadcast.
  The actor never awaits a socket write: holder writes go through a writer task
  with a bounded queue, so it always keeps reading holder output.
- The holder never blocks its control path on slow I/O. PTY input and client
  output each have a writer thread and a bounded queue; a full input queue
  answers `INPUT_BACKPRESSURE`, a client that falls too far behind is shut down
  and reattaches.
- A subscriber that falls behind gets a `session.snapshot` notification (screen
  plus `seq`) instead of the bytes it missed.
- One daemon per home: an exclusive `flock` on `daemon.lock`, taken before
  recovery. The daemon opens its socket only after it has reattached to the
  surviving holders. An unreachable holder marks its session `exited`.
- Holder EOF is not an exit. The session becomes `detached` in memory and stays
  `running` in the state file. The actor reconnects with backoff (50 ms up to
  2 s) and re-attaches from its last `seq`; it gives up only when the socket
  file is gone. A gap in `seq` sends subscribers a `session.snapshot`. Only
  `hold.exited` sets `exited`.
- `state.json` (and its temp file) and `daemon.lock` are mode `0600`.
- An exited session's actor ends once nobody is attached. The state file keeps
  the last 50 exited records.
- `state.json` is written to a temp file, fsynced, renamed, and the directory is
  fsynced. It carries `schemaVersion`; an unknown version stops the daemon. A
  corrupt file is renamed to `state.json.corrupt-<ts>` and the catalog is
  rebuilt by probing `sessions/*.sock` (command, title and size are lost).
- Directories the code creates get mode `0700`. An existing directory is never
  chmod-ed; it must be owned by the current user and not group/other writable.
- A holder reports a startup error (bad command, bad directory) on stderr and
  closes stderr once it runs, so `session.create` returns the real reason.
- Holder sockets live in a `0700` directory with mode `0600`. Env values and
  terminal output are never logged.
- Non-test code in `sushiai-daemon` and `sushiai-hold` denies
  `clippy::unwrap_used`.

## Deliberate step-1 shortcut

`Registry` (`sushiai-daemon/src/registry.rs`) is a `Mutex` around the session
catalog. The lock is never held across an await, and the state file is written
synchronously under it. The upgrade path is a catalog actor that owns the map
and the state file. Session state itself (screen, `seq`) is already in actors.

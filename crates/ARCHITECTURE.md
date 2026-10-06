# Crates

A Cargo workspace for the session daemon. It is not wired into the app yet. The
desktop app still uses Herdr and orchd; see `docs/architecture.md`.

## Shape

```
sushiai ──► sushiai-daemon ──► sushiai-core ──► sushiai-protocol
   │             └───────────► sushiai-agents
   ├──────► sushiai-hold ──────────────────────► sushiai-protocol
   └──────► sushiai-agents
```

- `sushiai-protocol`: wire types and the frame codec. No IO, no tokio.
- `sushiai-core`: pure session logic. Screen model over `vt100`, session
  transitions, state file model. No IO, no tokio.
- `sushiai-agents`: pure agent logic: hook payload parsing, the status state
  machine, launch specs, Codex `hooks.json` merging, screen heuristics. No
  tokio; the only IO is `codex_hooks::install`/`uninstall`.
- `sushiai-daemon`: tokio shell. Socket server, one actor per session, holder
  client, state file IO, holder spawning.
- `sushiai-hold`: synchronous holder. PTY, in-memory output ring, unix socket.
  Threads, no async runtime.
- `sushiai`: the binary. Parses arguments and calls the libraries.
  Subcommands: `daemon`, `status`, `hook EVENT` (run by agents), `hooks install|uninstall`,
  `hold` (internal, started by the daemon).

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

## Agent sessions and hooks

- `session.create {agent: "claude"|"codex"}` builds the command with
  `sushiai_agents::launch::build`; the hook command is the daemon's own binary
  (`current_exe`). `SUSHIAI_SOCKET`, `SUSHIAI_SESSION_ID`, `SUSHIAI_SESSION_TOKEN`
  reach the child through the holder's environment, never its argv.
- `sushiai hook EVENT` reads the hook JSON from stdin and sends `hook.event`
  on a connection that said `hello` with `role: "hook"`. That connection may
  call `hook.*` only and gets no notifications. The hook fails open: missing
  variables, a dead daemon, a wrong token or a timeout mean exit 0 and no
  output. Status events wait at most 2 s. `PermissionRequest` waits for the
  owner (daemon ask timeout 600 s, `SUSHIAI_ASK_TIMEOUT_MS` overrides it in
  tests) and prints the answer; no answer prints nothing, so the agent asks in
  its terminal.
- The token is random per session. The daemon keeps only its SHA-256, also in
  `state.json`, so it verifies the unchanged token of a surviving agent after a
  restart. A client never sees the hash.
- A session is in the catalog before its child starts (its first hook can beat
  the actor); a failed start removes it again.
- The session actor owns an `sushiai_agents::status::SessionStatus`. It stamps
  a per-session `seq` on each hook, and once a second runs the timers:
  hook silence (15 s while `working`), session-end grace (3 s), ask expiry.
  A holder exit feeds `Exit`. The agent status is `agentStatus` in the record;
  the process status stays in `status`.
- Asks: a `PermissionRequest` opens an ask (`session.ask`, listed in
  `session.list` as `asks`). `ask.respond` answers it. Timeout or a vanished hook
  process hands it back to the terminal (`PermissionClosed {decided: false}`).
  Asks are not persisted: they die with the daemon.
- `state.json` keeps `schemaVersion` 1: the new fields are optional with
  defaults, so an older file loads unchanged. A restored `blocked` status stays
  `blocked`: the agent may still show its dialog. The ask is gone, so the
  machine is told it went back to the terminal (`PermissionClosed {decided:
  false}`); the next prompt or stop clears it.
- `hook.event` carries the hook's `agent`. `sushiai hook` exits silently when
  `--agent` (default claude) differs from `SUSHIAI_AGENT`, and the daemon rejects
  a hook whose agent differs from the session's, so a nested agent that inherits
  the variables cannot report into its parent. `hook.event` is accepted only on
  `role: "hook"` connections.
- Limits: a hook payload over 1 MiB is not forwarded; at most 20 open asks per
  session (more get no decision and no ask); an ask's `input` on the wire is cut
  to 64 KiB of JSON text (the tool name is kept). `session.askClosed {askId,
  decided}` follows every ask that closes (answered, timed out, hook vanished,
  session exited).
- An exited session's token hash is cleared: its hooks are refused at once.
  Token compare folds all bytes of the SHA-256 digests (constant time).
- `<home>/bin/sushiai` is a symlink to the running binary. The daemon ensures it at
  start and `sushiai hooks install` ensures `~/.sushiai/bin/sushiai`; a stale link is
  replaced, a regular file is never overwritten. `hooks install` then writes
  `hooks.json` and Codex hook trust in `config.toml` (`codex_trust::trust`);
  `uninstall` removes trust first, then the hooks. `$CODEX_HOME` overrides
  `~/.codex`.
- Trust boundary: any process of the same user that can reach `daemon.sock` (for
  example an agent child that inherits `SUSHIAI_SOCKET`) can call `ask.respond`
  and the other client methods. The socket is mode 0600 and the token only
  separates sessions' hooks; this is not a security boundary on a single-user
  machine.

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
- `state.json` is written by one writer thread (the latest snapshot wins; an actor
  never fsyncs inline). Each write goes to a temp file, is fsynced, renamed, and
  the directory is fsynced. It carries `schemaVersion`; an unknown version stops the daemon. A
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
catalog. The lock is never held across an await or an fsync; the state file is
handed to a writer thread. The upgrade path is a catalog actor that owns the map
and the state file. Session state itself (screen, `seq`) is already in actors.

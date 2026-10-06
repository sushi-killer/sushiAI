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

## Catalog, notifications and daemon control

- **Replica rule.** The desktop is the master of projects and groups; a daemon
  holds only the projects that have a folder on its own host. The daemon's host
  name is `$SUSHIAI_HOST`, else the first line of `<home>/host`, else the machine
  hostname; `hello` returns it as `host`, and `projects.sync {host}` for another
  host fails with `INVALID_PARAMS` "daemon is <host>". `projects.sync {host, projects,
  full}` and `groups.sync {groups, full}` answer `{applied, ignored, rev}`. A partial
  sync merges by last writer (higher `rev`, then `updatedAt`). A `full` sync is
  authoritative: every record in the payload replaces the stored one whatever its
  `rev`, and live records absent from it become tombstones without a `rev` bump (so
  do the groups of a removed project). A project removed and added again is live
  with the next full projects sync, and its groups come back with the next full
  groups sync. The groups replica is write-only until a catalog read exists: nothing
  in the daemon reads it back. The replica is
  saved as `<home>/catalog.json` (mode 0600, own `catalogVersion` 1) by the same
  writer thread as `state.json`. A catalog file that cannot be read is renamed to
  `catalog.json.unreadable-<ts>` and the desktop syncs again. Bindings are opaque
  to the daemon (below), so a lost or unsynced catalog never blocks a launch.
- **Bindings.** `SessionInfo.project` / `group` are owned by the registry, as is
  `title`: `Registry::update` keeps the stored values, so an actor's stale copy
  never overwrites them. Bindings are opaque: `session.create {project?, group?}` and
  `session.update {id, project?, group?, title?}` store the ids as given and never
  check them against the catalog, because a fresh daemon, or one whose catalog file
  was set aside, may not have the desktop's projects yet. The desktop reconciles
  bindings. Only an absent `project` on create is filled in, from `project_for_cwd`.
  `session.update` leaves a missing field, clears on `null`, and returns the new
  `SessionInfo`.
- **Notifications** to every non-hook client: `session.created {SessionInfo}`
  (before any other notification about that session), `session.updated {SessionInfo}` (title or binding
  changed), `session.removed {id}` (`session.remove`, or the
  oldest exited record dropped past 50). `session.remove {id}` accepts only an
  exited session (`SESSION_STILL_RUNNING` otherwise).
- **Keepalive.** `$/ping` answers `{}` at any time. `hello.client` is a plain string
  for audit only.
- **Daemon control.** `daemon.shutdown` (no params) is refused on a `role: "hook"`
  connection. The daemon waits 200 ms so the response goes out, removes its socket,
  flushes the state and, once the runtime has stopped every task, flushes again and
  releases the lock. From the moment it is requested the daemon refuses requests that
  change anything (`SHUTTING_DOWN`: create, input, resize, close, update, remove and
  both syncs), so nothing is lost between the final flush and the exit. Holders and sessions are untouched. The
  daemon never starts a replacement: whoever connects next does (`sushiai proxy`
  over SSH, the desktop locally), so only clients start daemons and the `bin`
  symlink has one writer at a time.
- **Connector exit code** `sushiai_protocol::connector::DAEMON_DIED` (2) is what
  `sushiai proxy` exits with when the daemon closes first
  (`tests/connector-vectors/exit-codes.json` mirrors it).
- **Reconnect** (client side): a lost link is a new connector, a new `hello`
  (compare `daemon` and `capabilities`), `session.list`, and for each open
  terminal `session.attach`, whose result carries a snapshot and `seq`; output
  frames after it continue from `seq`. A `session.resync` or `session.snapshot`
  notification means the client missed events and replaces its view.
- **Log.** The binary logs at WARN by default; `SUSHIAI_LOG=info|debug|trace|off`
  changes it (`sushiai proxy` passes it on to an auto-started daemon).

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
  recovery; the holder of the lock writes its pid into the file. A reader must first fail
  `flock(LOCK_EX|LOCK_NB)` on the file before it trusts that pid: after a crash the
  file keeps the old number. The daemon binds
  its socket first (under `umask 077`, so it is never connectable by others, not
  even briefly) and answers `hello` while the holders reattach. Sessions of
  the state file that were running are listed at once as `detached` (still `running`
  in the file), so a reconnecting client never sees one vanish. As each holder
  answers, the session turns `running`; one that never answers turns `exited`.
  Either way `session.updated` announces the settled status. The holders
  reattach concurrently, each with its own 10 s limit. A request for such a session
  (`attach`, `input`, `resize`, `close`, `ask.respond`) waits for its holder (at most
  12 s) and then runs, so a client that reconnects at once loses nothing; after a
  timeout or an exit it gets the normal `SESSION_NOT_RUNNING`. A newer `state.json` schema is
  detected before the bind and stops the daemon.
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
- Directories the code creates get mode `0700`. An existing home (and its
  `sessions` directory) must be a real directory, not a symlink, owned by the
  current user; if group or others can enter it, the daemon tightens it to `0700`
  and goes on (an installer may have created it under umask 022). Another owner, a
  symlink or a file is refused.
- On a client's EOF the connection stops its stream tasks, drops its sender and
  waits (at most 2 s) for the writer to send what is queued: a client that
  half-closes after its last request still gets every answer. `sushiai proxy`
  relies on this: after stdin ends it half-closes the socket and waits for the
  daemon's close; it has no idle timer.
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

# Check speed measurement

No before/after benchmark was executed. The task's later Rules prohibit
whole-repo test suites and work in other checkouts, and reserve final checks
for the orchestrator. `npm run ci` is a whole-repo suite. This report supplies
no wall times or delta, and claims no speed improvement. No scratch worktrees
were created, so none require removal. There is no sandbox error from a
benchmark run, because no benchmark command was started.

The orchestrator can measure after committing this change, with fresh,
detached worktrees and an initially empty `target/` in each. Run `npm ci` in
each first; exclude installation from the timed command. `AFTER_COMMIT`
should be this task's committed HEAD, not the prerequisite HEAD. The baseline
is now `d8e32a0e5372a5bdca98f692b0e5ff5ae7d695ef`, the updated base onto which this task was carried;
comparing it with the final commit isolates this task's change.

```sh
git worktree add --detach artifacts/check-speed-before d8e32a0e5372a5bdca98f692b0e5ff5ae7d695ef
cd artifacts/check-speed-before
npm ci
env -u SCCACHE_DIR -u SCCACHE_SERVER_PORT -u SCCACHE_SERVER_UDS RUSTC_WRAPPER= /usr/bin/time -p npm run ci
```

An empty `RUSTC_WRAPPER` disables a wrapper configured in Cargo's user config;
merely unsetting the variable does not ensure a baseline without sccache.

Warm the shared cache in a separate fresh worktree:

```sh
git worktree add --detach artifacts/check-speed-warm "$AFTER_COMMIT"
cd artifacts/check-speed-warm
npm ci
env -u SCCACHE_SERVER_UDS RUSTC_WRAPPER=sccache SCCACHE_DIR="$HOME/.cache/sushiai/sccache" SCCACHE_SERVER_PORT=4227 sccache --start-server
env -u SCCACHE_SERVER_UDS RUSTC_WRAPPER=sccache SCCACHE_DIR="$HOME/.cache/sushiai/sccache" SCCACHE_SERVER_PORT=4227 npm run ci
```

Measure the after run in another fresh worktree:

```sh
git worktree add --detach artifacts/check-speed-after "$AFTER_COMMIT"
cd artifacts/check-speed-after
npm ci
env -u SCCACHE_SERVER_UDS RUSTC_WRAPPER=sccache SCCACHE_DIR="$HOME/.cache/sushiai/sccache" SCCACHE_SERVER_PORT=4227 /usr/bin/time -p npm run ci
```

Run each `git worktree add` from the task repo root; each `cd` is scoped to
that run. Then return to that root and remove only these scratch worktrees:

```sh
git worktree remove --force artifacts/check-speed-before
git worktree remove --force artifacts/check-speed-warm
git worktree remove --force artifacts/check-speed-after
```

| Measurement                                       | Result       |
| ------------------------------------------------- | ------------ |
| Before `real` seconds                             | Not measured |
| After `real` seconds                              | Not measured |
| Delta seconds (`before - after`)                  | Unavailable  |
| Delta percent (`100 * (before - after) / before`) | Unavailable  |

If a run fails, record its exact command, exit status and error before
reporting any timing as a successful full CI measurement.

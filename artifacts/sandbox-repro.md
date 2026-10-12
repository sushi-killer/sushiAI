# orchd task sandbox reproduction

Date: 2026-10-12. Repository: this task worktree, on
`task/feat-one-sandbox-policy-for-orchd-task-a`. Node: 26.8.1. Claude Code:
2.1.296. Codex: 0.160.1. Electron is installed through the worktree's linked
`node_modules`.

## Current reproduction

The original-profile `npm run ci` and `npm run test:desktop` each exited 1:
desktop assertions passed, but `/bin/ps` failed to execute even with
`ELECTRON_DISABLE_SANDBOX=1`. A read-only probe reproduced EPERM under an
allow-all Seatbelt profile. `/bin/ps` is a root-owned setuid binary.

The owner subsequently authorized removing all such calls inside this task.
The profile still has no process-execution exception. `sushiai proc <pid>`
uses macOS `proc_pidinfo` / `proc_pidpath` (Linux `/proc`) for process
identity. Native APIs also supply CPU samples, process state and run-wrapper
argv. The generic source `crates/process.rs` is compiled by the daemon and
task engine without introducing a dependency between core and a module.
Desktop cleanup asks its own daemon to run `daemon.shutdown` with
`closeSessions: true`, then waits for the PID from its own `daemon.lock` to
be gone. Default daemon shutdown continues to preserve sessions. Rust test
cleanup uses that RPC and owned ChildProcess handles; it no longer searches
process command lines or environments to signal a daemon or holder.
`tests/no-ps.test.cjs` prevents executable literals for the setuid utility in
shipped source and test helpers. The sandbox test's macOS paths and command
now have platform guards; the existing portability check is unchanged.

The authorized exact ignored real-repository reproduction ran outside the
managed sandbox (log `/private/tmp/orchd-native-repro-zf4lp9wb/real-repo.log`).
It exited 101 after `npm run ci` exited 1. Frontend/Cargo builds succeeded;
Node ran 1,455 tests (1,442 passed, 12 failed, 1 skipped). All twelve failures
were the existing conventions test detecting a missing explicit disposable
HOME in the new stopDaemon environment, rather than a sandbox denial. That
cleanup environment was corrected, and its targeted 19-test file passed.
The ignored test stopped at CI, so desktop and its last Cargo command were
not reached. Following the owner's selective-rerun instruction, the failed CI
and the two unreached commands run individually below through a helper linked
to the same current `sushiai_orch` library. The helper calls
`TaskSandbox::build` and `task_sandbox_profile` and supplies `check_env`; it
has no independent profile rules or cache list. No check command or package.json
was edited. Reserved verification commands remain the orchestrator's work.

## Final native reproduction results

The final source removes every executable process-list utility literal in
shipped code and test helpers. The profile has no process-exec exception.
The original `/bin/ps` execution denial is resolved by native inspection and
owned-process shutdown. No new filesystem, Unix-connect, Mach-service,
network, Electron/GPU or Keychain denial was reported by these repository
reproductions.

Logs and rendered profiles are retained in
`/private/tmp/orchd-native-repro-zf4lp9wb/`. The helper renders both SBPL and
check environment from the current library's policy. Check commands and
assertions are unchanged; package.json is unchanged in this task.

| Updated-policy command                            | Exit | Duration | Result                                                                                                                                                                                                                                                                                                     |
| ------------------------------------------------- | ---- | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `npm run ci`, first selective rerun               | 1    | 323.21 s | Node: 1,455 tests; 1,451 passed, 3 failed, 1 skipped. Failures: artifact-skill removal with a shared custom config home, local-module disable state, fake-SSH hello deadline. No sandbox denial.                                                                                                           |
| `npm run ci`, fresh state and foreground priority | 1    | 112.20 s | Node: 1,455 tests; 1,453 passed, 1 failed, 1 skipped. Only `tests/daemon-remote.test.cjs`'s `proxy exit 2 reconnects at once` failed: `timed out waiting for first ready` after 60 s. Its fake proxy dies after 150 ms. No sandbox denial. CI stops before Rust suites, lint and format checks.            |
| `npm run ci`, final fresh-state reproduction      | 101  | 884.96 s | Node: 1,455 tests; 1,454 passed, 0 failed, 1 skipped. Workspace formatting and Clippy passed. Rust reported seven daemon-restart failures, three nested-Seatbelt unit failures, and one pre-existing-check assertion failure. Six Rust targets failed; CI stopped before conventions, ESLint and Prettier. |
| `npm run test:desktop`                            | 0    | 211.27 s | All hidden-window, renderer/interaction assertions and daemon cleanup passed; companion smoke returned `{"companion":"ok"}`. `ELECTRON_DISABLE_SANDBOX=1` came from policy.check_env.                                                                                                                      |
| `cargo build -p sushiai`                          | 0    | 4.15 s   | Standalone real-repository build passed under the same task profile.                                                                                                                                                                                                                                       |

The CI convention failure from the exact ignored test was fixed by setting
HOME explicitly to the script's disposable profile parent during daemon
shutdown. Its targeted 19-test file then passed. The artifact-skill test's
custom-config-home failure was eliminated by removing reproduction-only
CLAUDE_CONFIG_DIR / CODEX_HOME overrides while keeping HOME disposable.
All four later failing Node tests passed individually under the native
profile, without editing a test or timeout: artifact-skill disable (1),
local-daemon module switch (1), remote task/hello (1), and exit-2 reconnect
(1, 272 ms). Isolated successes and the 150 ms fixture suggest load-sensitive
suite timing; they do **not** make a failed full CI exit successful. All
failed reproduction results are retained.

The desktop stop condition is resolved: desktop exits 0 with the policy's
Electron variable, with no process exception. **Full CI has not passed in
this reproduction.** The final CI invocation reached the Rust suites and
exposed the issues detailed below. No check command or assertion was weakened.

### Rust failures reached by the final CI reproduction

The seven restart tests failed at `tests/common/mod.rs` with `no response
to hello`. The corrupt-state restart also failed alone under the native
profile (exit 101, 15.76 s), but passed without an outer profile (exit 0,
2.90 s). A temporary diagnostic copy with child stderr visible and a
separate owned-process restart probe both succeeded under the profile.
The faster native process probe exposed a readiness race: the helper
considered any connectable socket ready, including the previous daemon's
socket. The helper now requires its own spawned child's PID in `daemon.lock`
as well as a connectable socket. This strengthens process identity without
changing the profile, check commands or assertions.

All seven original failures then passed individually under the same native
profile, through targeted `cargo test -p sushiai --test <file> <name> --
--exact --nocapture` commands:

| Test file                             | Exact test                                                                       | Exit | Wall time |
| ------------------------------------- | -------------------------------------------------------------------------------- | ---- | --------- |
| `daemon`                              | `a_corrupt_state_file_is_set_aside_and_sessions_are_found_again`                 | 0    | 11.23 s   |
| `agents`                              | `status_and_a_blocked_ask_survive_a_killed_daemon_and_the_token_is_never_stored` | 0    | 4.92 s    |
| `launch_options_attach_read_and_open` | `the_same_idempotency_key_returns_the_same_session_even_after_a_restart`         | 0    | 9.07 s    |
| `lifecycle`                           | `the_catalog_and_bindings_survive_a_daemon_restart`                              | 0    | 6.48 s    |
| `lifecycle`                           | `a_reattach_at_the_same_size_does_not_resize_the_child`                          | 0    | 5.19 s    |
| `lifecycle`                           | `a_recovered_screen_replays_output_at_the_sizes_it_was_written_at`               | 0    | 2.96 s    |
| `lifecycle`                           | `the_socket_answers_while_a_holder_is_slow_to_reattach`                          | 0    | 10.49 s   |

The `pre_existing` integration's
`a_check_that_fails_once_on_the_base_and_then_passes_is_not_asked_about`
failed its flaky-base decision assertion even though the task reached
`done`. It passed unchanged when run alone under the same profile (exit 0,
7.23 s). This isolated success does not erase the suite failure.

Three existing library tests use a native `test_app` fixture and try to
apply another Seatbelt profile inside CI's task profile:

- `engine::tests::an_amendment_made_while_a_parent_loop_is_live_is_applied_before_its_check`
- `engine::tests::an_amendment_pending_before_the_base_preflight_replaces_the_checked_command`
- `engine::verify::grounded_checks_tests::a_baseline_undoes_what_the_checks_did_to_the_worktree`

An outside-sandbox diagnostic using the policy as its outer profile and
`(version 1)(allow default)` as its inner profile exited 71 with exactly
`sandbox-exec: sandbox_apply: Operation not permitted`. Its log is
`nested-seatbelt.log`. This is the same nesting restriction that the
integration helpers already detect, and no SBPL path or process exception
can lift it. All three exact unit test names passed from an unsandboxed
parent, allowing their own native check profiles to apply (exit 0 each;
0.83 s, 0.95 s and 1.69 s). No full reserved library command was run.

The owner approved a host fixture for these three behavioral tests only when
native sandbox application is unavailable. The shared helper used by
`tests/common` also powers the library fixture, prints the reason, and changes
only the test app's saved sandbox setting; all three tests retain their
assertions. This does not change production sandbox policy or check commands.
Decision: these tests cover amendments and baseline restoration, so the
sandbox mode is incidental; nested Seatbelt is unavailable in CI and they
run with host mode only in that environment. Outside a nested sandbox they
keep the native default. All three targeted tests passed with the host
fallback and printed `sandbox-exec cannot apply a profile (nested or
unavailable Seatbelt)`. The ignored real-repo test was not repeated.

## Execution environment and baseline commands

The managed-shell prerequisite probe was:

```sh
/usr/bin/sandbox-exec -p '(version 1)(allow default)' /usr/bin/true
```

It exited **71** with:

```text
sandbox-exec: sandbox_apply: Operation not permitted
```

The child never starts: this environment cannot nest Seatbelt profiles. The
fix is to execute the reproduction from an unsandboxed shell, not to widen
an inner profile. The outside-sandbox allow-all probe exited **0**.

The approved baseline used `/usr/bin/taskpolicy -b` and the exact original
`verify.rs` profile shape, reconstructed from HEAD:

```text
(version 1)
(allow default)
(deny file-write* (subpath "/"))
```

It then allowed writes under the current worktree, own run directory,
`/dev`, disposable `~/.npm`, `~/.cache`, `~/.cargo/registry`,
`~/Library/Caches`, `TMPDIR`, `/private/var/folders` and `/private/tmp`, as
`verify_allow_write_paths` originally did. Reads were unrestricted. The
baseline did not add Git-common-dir, Rustup-home or linked-dependency grants.

HOME, SUSHIAI_HOME, CODEX_HOME, CLAUDE_CONFIG_DIR, npm cache and TMPDIR were
disposable. API credential variables were removed. Installed Cargo
`bin/registry/git` and Rustup `toolchains` were symlinked into disposable
build homes for read access; Rustup settings were copied.
`RUSTC_WRAPPER` was empty because the installed sccache wrapper had already
failed with EPERM before compilation. No npm command, `package.json` or
script was edited. `SUSHIAI_TEST_WINDOW=hidden` and
`ELECTRON_DISABLE_SANDBOX=1` were set in the check environment.

Baseline logs and result JSON are retained at
`/private/tmp/orchd-baseline-1xucxtnt/`. The following timings are wall-clock
measurements, not claims that the whole commands passed:

| Original-profile command                 | Exit | Duration | Result                                                                                                                                                                          |
| ---------------------------------------- | ---- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `npm run ci`                             | 1    | 94.42 s  | Frontend build and Cargo build succeeded. Node tests: 1,453 total, 1,444 passed, 8 failed, 1 skipped. The chain stopped before `test:daemon`, conventions, lint and formatting. |
| `npm run test:desktop`                   | 1    | 70.18 s  | Frontend build and all `smoke.mjs` rendering/interaction assertions passed. Cleanup failed executing `ps`; `smoke-companion.mjs` was not reached.                               |
| `cargo build -p sushiai` (prior attempt) | 0    | 15.57 s  | Real repository build succeeded under the original profile.                                                                                                                     |

The six Node failures in catalog sync, daemon sizing, daemon restart, fake
SSH, terminal streaming and the deadline helper occurred at
`tests/helpers/deadline.cjs:47`, where the failed `spawnSync("ps", ...)`
left `stdout` undefined and `.trim()` threw. A separate portability failure
identified macOS-specific code in the new `tests/sandbox.rs` without nearby
`cfg(target_os = "macos")` guards (lines 89, 164, 270, 375 and 517 at the time
of this run). The daemon-survival test also failed its `no requeue`
assertion (`2 !== 1`). The latter two are assertion failures, not reported
filesystem, Mach-service or network denials. They were recorded rather than
changing checks to make reproduction pass.

## Exact desktop failure and process probe

The desktop output included three successful hidden-window assertions and:

```text
Hidden window: 0 visible, none focused, dock hidden, viewport 1380x880
"passed": true
"screenshot": "artifacts/workspace.png"
```

The exit status was nevertheless **1**, from cleanup:

```text
Error: spawnSync ps EPERM
    at stopDaemon (.../scripts/lib/daemon-binary.mjs:26:17)
    at .../scripts/smoke.mjs:912:3
  errno: -1,
  code: 'EPERM',
  syscall: 'spawnSync ps',
  path: 'ps',
  spawnargs: [ '-axo', 'pid=,command=' ],
  status: null,
  pid: 0,
  stdout: undefined,
  stderr: undefined
```

The diagnostic ran only this read-only child process operation:

```js
spawnSync("/bin/ps", ["-o", "lstart=", "-p", String(process.pid)], {
  encoding: "utf8",
});
```

Both an allow-all profile and the original profile returned:

```json
{
  "status": null,
  "error": { "code": "EPERM", "message": "spawnSync /bin/ps EPERM" }
}
```

Thus there is an execution denial independent of the original write list.
No Mach-service name or filesystem path denial appeared in these logs.
The disposable smoke daemon left by the failed cleanup was identified by its
own temporary daemon lock, start time and this worktree's executable. It
received SIGTERM outside the profile, and its exit was confirmed. Other
daemons and processes were preserved.

## Denial inventory and policy rules

Only observed denials are described as reproduced. A successful build or
smoke assertion does not prove access to every cache location.

| Area                                                            | Observation                                                                                                                                                            | Policy rule or disposition                                                                                                                                                                                 |
| --------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Applying a nested Seatbelt profile                              | Managed-shell allow-all probe: exit 71, `sandbox_apply: Operation not permitted`.                                                                                      | Execute native reproductions outside the managed sandbox; no inner SBPL rule can grant nesting.                                                                                                            |
| `/bin/ps` execution                                             | Baseline desktop cleanup: `spawnSync ps EPERM`; targeted allow-all and original-profile probes reproduce it. CI's six process-helper failures have the same call site. | Owner authorized removing every call: native process identity/CPU APIs, daemon-owned holder shutdown, and ChildProcess cleanup. No profile exception was added.                                            |
| Synthetic denied worktree parent metadata                       | Earlier native synthetic CI: Cargo dep-info generation failed with `Operation not permitted (os error 1)` under `.task-worktrees/task-sandbox/target/debug/deps`.      | `sandbox.rs` derives literal ancestors of the own worktree/run; SBPL allows `file-read-metadata` on those literals only. The native task subsequently reached done. Directory contents remain denied.      |
| Worktree and Cargo target                                       | Real Cargo passed under original and new profiles; baseline frontend build also passed.                                                                                | Worktree is a write root. No external shared target directory was introduced.                                                                                                                              |
| Git common dir and `.git/worktrees/<name>`                      | Synthetic linked-worktree `git status` and `git add` passed outside the managed sandbox. No real-repo Git-metadata denial was reported.                                | Resolve the worktree's `.git` pointer and `commondir` into write roots.                                                                                                                                    |
| Linked `node_modules`                                           | Baseline Vite build succeeded with the linked directory. No dependency-write denial was reported.                                                                      | Add its resolved target only if it matches the current repo's main checkout `node_modules`.                                                                                                                |
| `~/.cargo/git`, registry and Cargo state                        | Real builds used installed sources through read-only links and disposable state; no Cargo source-cache denial was reported.                                            | CARGO_HOME, or `~/.cargo`, is a write root. Tests do not grant writes to installed source-cache symlink targets.                                                                                           |
| Rustup home                                                     | Real builds passed with disposable Rustup state and read-only installed toolchains.                                                                                    | RUSTUP_HOME, or `~/.rustup`, is a write root.                                                                                                                                                              |
| npm cache                                                       | No npm-cache denial was reported by the baseline.                                                                                                                      | Configured npm cache, or `~/.npm`, is a write root.                                                                                                                                                        |
| `ms-playwright` and other caches                                | Installed Playwright launched hidden Electron; no cache-path denial was reported. No browser download was needed.                                                      | `~/.cache` and `~/Library/Caches` are write roots.                                                                                                                                                         |
| Own run, TMPDIR, `/private/var/folders`, `/private/tmp`, `/dev` | Native synthetic write assertions passed. No such real-repo denial was reported.                                                                                       | Own run and canonicalized temporary/cache paths are write roots; own run is a Seatbelt read exception.                                                                                                     |
| Electron userData                                               | Real `smoke.mjs` used its disposable test profile and passed rendering/interaction assertions.                                                                         | Existing launcher profile lies under allowed `/private/tmp`; launcher was unchanged.                                                                                                                       |
| Electron/GPU nested Seatbelt                                    | With `ELECTRON_DISABLE_SANDBOX=1`, real hidden-window and renderer assertions passed. There was no renderer/GPU denial in the output.                                  | Policy check environment sets `ELECTRON_DISABLE_SANDBOX=1`; outer task sandbox remains active. The baseline exited 1 at the `ps` cleanup operation. The owner then authorized replacing that prerequisite. |
| Mach services                                                   | No service-name denial was reported.                                                                                                                                   | Retain `(allow default)`; no service exception invented.                                                                                                                                                   |
| Network                                                         | Codex reached OpenAI through WebSocket/HTTPS and returned 401 with disposable credentials absent. No network denial was reported.                                      | Checks and Codex task shell commands retain open network. Claude retains `allowedDomains`.                                                                                                                 |
| Keychain                                                        | No owner credentials were copied or Keychain command invoked. Codex used file-based credential storage. No Keychain denial was reported.                               | No Keychain-specific exception added.                                                                                                                                                                      |
| Other project file and `.env`                                   | Native synthetic reads failed with EPERM/EACCES.                                                                                                                       | Deny other known project roots; for a registered ancestor, deny its other children.                                                                                                                        |
| Other task run, default/configured/retained worktrees           | Native synthetic reads of other-repo and same-repo worktree secrets failed after the regression fix.                                                                   | Deny data_dir, default worktree storage, configured roots and stored tasks' actual worktree paths, with own worktree/run read exceptions.                                                                  |
| `~/.claude/projects`, `~/.codex/sessions`, archived sessions    | Native synthetic reads failed with EPERM/EACCES.                                                                                                                       | Explicit read denies, retained even though Codex's state home is writable.                                                                                                                                 |
| Daemon Unix socket                                              | Native synthetic connection failed with EPERM, including symlink and `/tmp` aliases. No real daemon RPC was sent in the isolation probe.                               | Deny `network-outbound` to the specific canonical Unix socket.                                                                                                                                             |

## New-policy real Cargo and Codex results

Earlier attempts built a disposable Rust helper linked to `sushiai_orch`
and rendered the profile through `TaskSandbox::build` and
`engine::task_sandbox_profile`, rather than maintaining another new-policy
write list.

| Run                                                               | Result                                   | Observation                                                                                                                                                                                                 |
| ----------------------------------------------------------------- | ---------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| New task SBPL, real `cargo build -p sushiai`                      | Exit 0; 0.13 s                           | Reused the original build's target output; no sandbox denial.                                                                                                                                               |
| New task SBPL, real Codex 0.160.1                                 | Exit 1                                   | Started with `danger-full-access`; reached OpenAI via WebSocket/HTTPS; HTTP 401 because disposable home contains no credentials. No Seatbelt denial.                                                        |
| Exact ignored real-repo test, earlier managed-shell invocation    | Exit 0, skipped                          | Printed `SKIP sandbox: sandbox-exec cannot apply a profile (nested or unavailable Seatbelt)`; no repository suite ran.                                                                                      |
| Exact ignored real-repo test, approved outside-sandbox invocation | Exit 101; 738.51 s including compilation | CI exited 1 on stopDaemon's missing explicit HOME convention. No sandbox denial; desktop and the final Cargo command were not reached. The cleanup environment was fixed and checks were selectively rerun. |

The Cargo and Codex runs used disposable HOME, SUSHIAI_HOME, CODEX_HOME, npm
cache and TMPDIR, empty RUSTC_WRAPPER, and read-only links to installed Cargo
sources/Rustup toolchains. The target directory was in this worktree. The
Codex probe removed API credential variables and avoided Keychain:

```sh
sandbox-exec -p "$new_profile" codex exec \
  --sandbox danger-full-access --skip-git-repo-check \
  -c 'cli_auth_credentials_store="file"' 'Reply with OK.'
```

The 401 proves startup and network access under the profile, not an
authenticated model completion. Synthetic daemon tests prove completion and
checks through orchd's actual Codex wrapper.

The socket has no RPC token check. Native Codex implements therefore retain
the socket deny and omit `sushiai-messages`: no peer messaging or
`ask_orchestrator`. Incoming messages reach the next attempt brief. A
`sandboxRepos` host entry restores the bridge. No binary-specific daemon
socket exception was introduced. Codex shell commands have open network
even with `codexNetwork=false` in `danger-full-access` mode.

## Claude schema and read-tool regression

The installed Claude Code 2.1.296 schema contains optional string arrays
`sandbox.filesystem.allowWrite`, `denyRead`, and `allowRead`. `allowRead`
re-allows paths beneath filesystem denied roots. The task renderer emits
all three, plus permissions denies `Read(//<absolute-path>/**)` alongside
the existing Bash commit/push denies. Non-task settings retain their
original renderer.

The network schema provides `allowUnixSockets` and `allowAllUnixSockets`,
with grants merged across settings, and has no deny-Unix-socket key. Native
Claude implements exclude project/local settings sources so those grants
cannot widen generated policy. Claude starts MCP servers and Stop/
PreToolUse hooks outside its shell sandbox, preserving their bridge.

The earlier Read-tool regression exited **101** because whole worktree
storage roots in `permissions.deny` matched the task's own `src/main.rs`.
A filesystem `allowRead` does not override a tool permission deny.
`sandbox.rs` now owns `read_tool_denied_roots`, recursively enumerating
other children along the own worktree path. Daemon data, including own run
credentials, remains denied to the Read tool; only a worktree under daemon
data is excluded. Symlink children are resolved, while ancestor aliases
are not traversed recursively. Seatbelt retains whole-root denies, own
read exceptions and literal ancestor metadata exceptions.

## Targeted tests recorded by earlier attempts

The following targeted native tests ran outside the managed sandbox with
no skips:

- Foreign-project/session/run/socket isolation passed. A regression first
  failed when another repo's `.env` under `<SUSHIAI_HOME>/worktrees.noindex`
  was readable; default/configured/retained worktree denies fixed it.
- Worktree, linked Git metadata, own run, temporary paths and caches passed.
  Directory listing of denied storage parents still failed as expected.
- The native synthetic daemon task reached `done` after unchanged synthetic
  Cargo/Node CI and hidden Electron checks. Its copied foreign worktree
  secret remained unreadable, including after `worktreeRoot` changed.
- The host-override daemon task reached `done`, without the profile or
  policy-added Electron variable, with messaging restored and global
  `sandbox` still native.
- After the Claude Read-tool fix, targeted policy units passed **7 tests**,
  profile integrations passed **3 tests**, and the expanded exact Claude
  Read-tool regression passed **1 test**. Other targeted harness tests
  covered native check environment, Codex argv/wrapper, task MCP grants
  and stable prompt hashing.

Managed-shell profile tests print their skip reason. Their successful test
process exit is not additional isolation proof. Earlier continuation work also passed the native process unit (1 test), shutdown
with owned holders regression (1 test), proxy integration file (10 tests),
Node deadline helper (9 tests), portability and no-process-utility guards,
and the conventions test file (19 tests after the cleanup HOME fix). The
shutdown test first failed to start the daemon in the managed sandbox, then
passed outside it using its disposable daemon home. The initial compilation
attempt hit sccache EPERM in the managed shell; RUSTC_WRAPPER was empty for
the targeted and reproduction builds. Rustfmt, installed Prettier and
`git diff --check` completed. No reserved verification command or commit was
run outside the explicitly authorized reproduction.

The latest continuation strengthened `tests/no-ps.test.cjs` to reject the
utility's absolute paths and quoted shell-command strings too; its targeted
test passed (1 test). Daemon-control documentation now describes the
opt-in session-holder shutdown. Only the test-owned daemon startup helper,
the guard and these documents changed during this continuation.

The final Rust reproduction exposed an sccache startup denial: after the
daemon's sccache server idled out, a sandboxed Cargo invocation tried to
start it under Seatbelt and failed at `rustc -vV` with `Operation not
permitted (os error 1)`. The attempted per-run `sccache --show-stats` probe
and server restart did not fix it, even after orchd was restarted with that
change. Base commit `e0ab61af` later set `RUSTC_WRAPPER=""` for native checks.
This task moved that behavior into `TaskSandbox.check_env`, alongside
the policy-owned `SCCACHE_` removal prefix. The only Rust wrapper override is
`RUSTC_WRAPPER=""`; Cargo's configured wrapper is superseded by that variable.
The env is applied to native checks and implement runs; host checks keep the
existing optional sccache wrapper, and sccache starts normally when host Cargo
uses it. The sandbox never grants a compiler-cache exception.

Decision: the unsuccessful per-run sccache server probe was removed. Host
checks keep Cargo's existing configured wrapper, which can start sccache as
needed; native checks use the policy-owned empty `RUSTC_WRAPPER`.

After the final policy/env consolidation, the targeted sandbox integration
file passed (5 passed, 1 ignored). Its five native-profile tests printed the
nested-Seatbelt skip reason in the managed shell. The exact ignored real-repo
test was also run outside that shell with disposable test state. It exited 101
after `npm run ci` exited 1 (234.39 s): frontend build and `cargo build -p
sushiai` succeeded; Node had 1,448 passes, 6 failures and 1 skip. Four failures
reported Unix socket paths longer than `SUN_LEN`; two daemon readiness tests
timed out. No sandbox denial appeared in these failures. The ignored test
stopped at CI, so its later `npm run test:desktop` and standalone Cargo command
were not reached. The earlier direct new-profile desktop smoke and Cargo build
both exited 0 (table above); no later desktop failure occurred to trigger the
owner's stop condition. Codex under the new profile started and reached the
OpenAI endpoint, then exited 1 with HTTP 401 because the disposable home had
no credentials, not because Seatbelt denied it.

After the final policy env cleanup, the focused
`sandbox::tests::check_env_is_applied_only_to_native_checks` unit test passed
(1 passed, 0 failed). This edit only removed the redundant
`CARGO_BUILD_RUSTC_WRAPPER=""` setting. The full CI / desktop reproductions
above were not repeated for this env-only correction; the orchestrator owns
final checks.

## Scope

Task-local `package.json` remains unchanged. The only task-local script change
is `scripts/lib/daemon-binary.mjs`, as authorized by the owner's amended
criterion. The literal comparison with `main` includes an inherited
postinstall change in `package.json`, which was preserved. Architecture boxes
and arrows did not move; no screen changes were made. The desktop
reproduction generates its normal ignored screenshots.

No commits, pushes, PRs, packaging or reserved verification commands were
run. Full verification, review and commit belong to the orchestrator.

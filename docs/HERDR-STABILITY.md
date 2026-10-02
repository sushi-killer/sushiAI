# Herdr integration: implementation and measured evidence

sushiAI installs the official Herdr 0.9.3 release and works with any Herdr
whose API schema still offers everything the contract names (`minProtocol` 20,
the schema was checked on 0.8.2 and 0.9.3; the live install evidence under
`docs/verification/herdr/` is still from 0.8.2). A version or protocol number
alone never blocks a session. The terminal CLI must speak its daemon's
protocol: when the pinned CLI does not, the owner's own CLI and then any
earlier release sushiAI installed are tried. The owner excluded the
experimental Herdr patch; no fork, patched binary, or patch is shipped by this
change. Herdr owns running processes and terminal sessions. sushiAI owns their
presentation, layouts, launch preparation and user actions. Conversation state
and the task orchestrator retain their existing owners.

## Changes and acceptance evidence

| Area               | Result                                                                                                                                                                                                                                                                                                         | Proof                                                                                                                                                                                                                                                    |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Identity           | Versioned workspace and panel IDs include the endpoint. Migration updates layouts, selections, views, merged layouts, chat focus and closed-project references before the first render. Local panels and conversations remain separate.                                                                        | Migration regressions; real hidden Electron with two isolated sockets both reporting `w1`/`w1:p1`; the selected terminal and a local files pane retain a 62/38 split.                                                                                    |
| Checkout matching  | Paths are canonicalized on their owning host. Saved checkout paths and the durable creation journal survive shell `cd`, closed original panes and app restarts. Worktrees and separate checkouts retain separate workspaces.                                                                                   | Local and real SSH lifecycle fixtures; canonical aliases, separate worktrees and legacy saved-checkout regressions.                                                                                                                                      |
| Operation identity | Queues are registered before asynchronous work. A new intent gets a new operation ID; replaying an ID reuses its result. Creation is journaled before agent preparation. An explicit preparation retry uses the created pane.                                                                                  | 20 distinct concurrent requests produce one workspace and 20 panes; 20 repeated requests add zero panes, locally and through SSH. Real renderer-to-main IPC repeats the same acceptance case.                                                            |
| Preparation        | Creation, addition and restoration share the same host-aware preparation. Process variables use native Herdr `env`; account and file settings are staged separately. Read or transfer failures surface with a retry action.                                                                                    | Real terminals and a deterministic agent executable receive the same settings in checkout and worktree, locally and through SSH; injected preparation failure and service restart retain the original pane. No secret is stored in the creation journal. |
| Errors             | Tagged IPC responses are reconstructed as structured errors in the renderer. Recovery uses codes rather than message text or a React delay.                                                                                                                                                                    | Real Electron preserves `pane_not_found`; bridge and restoration regressions.                                                                                                                                                                            |
| Snapshots          | Each endpoint has a single-flight coordinator with coalescing, trailing refresh, invalidation and connection generations. Native Herdr events trigger snapshots; reconnect triggers a complete snapshot. Control reconciliation runs every 60 seconds.                                                         | Deferred-response and reconnect regressions; counted real RPCs and external changes in the built app.                                                                                                                                                    |
| Compatibility      | One contract pins release, protocol, source commit, capabilities and artifact checksums. Daemon and stream CLI are checked separately before attachment, locally and through SSH. Managed installation uses the pinned release and verifies its SHA-256.                                                       | Real 0.8.2 daemon/protocol 20 and CLI/schema/control checks; checksum rejection and managed-install regressions; official local and fresh SSH release downloads pass verification.                                                                       |
| Terminal delivery  | Output receives bounded credit and acknowledgement only after xterm processes it. Input, packet count and frame parsing are bounded. Invalid frames disconnect; valid final output drains before teardown. Reattachment waits for the previous CLI to release its controller and refreshes the terminal image. | Stream regressions, immediate local/SSH reattachment, input/paste/selection/Unicode checks and the 600-second real Electron/xterm load run below.                                                                                                        |

Existing live sessions in the same checkout remain accessible. This change does
not merge or terminate them. Ended-record cleanup retains the existing policy.

## Before and after

Measurements were recorded on 2026-10-01, Linux x64, Intel i7-11800H, Node
22.23.2 and Electron 44.2.0. Both application variants use the same official
Herdr 0.8.2 binary. The baseline is commit `113f576`; the updated app is built
from implementation commit `ae6de54`, after integrating that baseline. The final
branch also preserves main commit `9f45d06` for Git-over-SSH preparation recovery;
that later integration does not change the Herdr snapshot or terminal-flow
modules. Timings below describe the recorded pair, not a new benchmark after
that integration. SSH uses a real encrypted connection over loopback with an
isolated sshd and profile; it does not represent WAN latency.

| Measurement                                        |  Before: `113f576` |                                                 After | Interpretation                                                                       |
| -------------------------------------------------- | -----------------: | ----------------------------------------------------: | ------------------------------------------------------------------------------------ |
| External workspace to sidebar DOM, local p50 / p95 | 2231.4 / 3412.2 ms |                                      211.5 / 287.4 ms | 8 external changes per variant; direct renderer timestamps.                          |
| External workspace to sidebar DOM, SSH p50 / p95   | 3196.9 / 3664.3 ms |                                      205.3 / 356.8 ms | 8 external changes per variant through actual SSH.                                   |
| Full snapshots during 120 seconds of idle, local   |                 30 |                                                     2 | Measured reduction: 93.33%; exceeds the 90% target.                                  |
| Full snapshots during 120 seconds of idle, SSH     |                 30 |                                                     2 | Measured reduction: 93.33%; excludes startup and mutation phases.                    |
| Output-credit bound per stream                     |       Not measured | 262,144 bytes enforced; 189,783 bytes sampled maximum | This is a bounded-queue result, not a measured memory reduction against the old app. |
| 20 distinct launches, then 20 replays              |       Not measured |                  1 workspace, 20 panes, 0 extra panes | Behavioral acceptance locally, through SSH and through actual Electron IPC.          |

The 500 ms objective is met for sidebar DOM readiness in all 8 updated samples
per transport. Physical paint latency remains a target, not a proven result:
hidden-window animation-frame p95 was 1209.2 ms locally and 1203.5 ms through SSH, and Playwright
locator p95 291.8 / 386.0 ms locally / through SSH. Those timestamps remain in the raw records.
The DOM measurement uses a `MutationObserver` and confirms positive element
geometry; it is not a claim about visible monitor paint. This small sample is
acceptance evidence, not a production latency distribution.

After an injected 500 ms connection break, the complete list returned in
1785.9 ms locally and 1394.9 ms through SSH. Closing the app preserved the
exact Herdr shell PID on both transports. No before/after improvement is
claimed for either check.

## Ten-minute terminal load

The default load driver uses a real hidden Electron app, production terminal
IPC, actual xterm write acknowledgements and two real Herdr panes. It changes
ANSI output and Unicode continuously, exercises paste, and holds real text
selections for about 15 seconds every 30 seconds. It does not force garbage
collection. This load run preceded the final main integration; terminal flow and output-queue code remained unchanged during integration. The run processed 102,951,198 UTF-8 bytes and sampled for
599.987 seconds (560 observations).

RSS uses decimal MB. The comparison is between equal early and late windows
of the **updated implementation after a 63.802-second warmup**, not between
two application versions.

| Measurement          | Early window, 64.871–198.367 s | Late window, 467.122–599.987 s |
| -------------------- | -----------------------------: | -----------------------------: |
| Main RSS, mean       |                     169.236 MB |                     171.941 MB |
| Renderer RSS, mean   |                     143.848 MB |                     148.322 MB |
| Main heap used, mean |                       7.869 MB |                       7.022 MB |

Main/renderer RSS drift was +2.705/+4.474 MB, within the declared test
threshold of 16 MiB. Main heap declined by 0.846 MB. Each stream CLI peaked
at 6,631,424 bytes RSS. This run passed its plateau check; it does not prove
zero memory growth for every workload. Daemon memory was not measured.

The sampled maximum output credit and renderer pending data were both
189,783 bytes, below the enforced 262,144-byte credit. Sampling is once per
second and may miss shorter peaks; the independent credit invariant tests
verify the hard bound. A decoded frame may additionally occupy up to 4 MiB;
the encoded line parser has a 5,658,968-byte bound. Main input has a 2 MiB/1024-message
limit; renderer input has a 1 MiB/1024-message limit. No arbitrary output fragments are discarded to meet those bounds.

Unexpected CLI exit retains valid pending output until acknowledgement. A
500,000-byte regression verifies complete final delivery. Both live sessions
also survived closing the load-test app. These are correctness checks, not a
baseline throughput or memory gain claim.

## Verification and limits

Final `npm run ci` passed: 1056 Node tests and 761 Rust tests, with Rust
1.94.1 formatting/clippy, conventions, ESLint (0 errors, 42 warnings),
Prettier and diff checks passing. Final desktop smoke passed with no skips
and no renderer errors. The exit-code verdicts are recorded in `artifacts/herdr-ci-final.log` and
`artifacts/herdr-desktop-final.log`. The release fragment is
`docs/releases/unreleased/herdr-session-stability.md`; the ContextBridge
error-transport lesson is recorded in `docs/LESSONS.md`.

The dedicated checks cover real Herdr RPC, terminal streaming, immediate
reattachment, Unicode and backspace, terminal input, scrolling, selection,
paste, and resize. UI evidence shows the migrated selected panel, unchanged
split geometry, preserved conversation data and separate daemon/CLI status.
The generic desktop smoke can skip checks when its default Herdr socket is
absent; the separate isolated Herdr UI fixture supplies those checks.

The agent-preparation fixture uses a deterministic agent executable, not a
paid external model or account login. Linux checks do not establish a macOS
runtime result. The managed local installer downloaded and verified the
official release successfully. A final clean installation through real SSH
also downloaded the exact official asset, verified its SHA-256, reused the
verified installation on repeat, and passed independent daemon/CLI checks.
Earlier download attempts hit network timeouts and surfaced errors; those
failed retries are retained separately. Reproduction requires access to the
pinned GitHub release asset.

All claimed synchronization gains are measured above. WAN performance,
visible physical paint within 500 ms, and memory reduction versus the baseline
remain unverified. Herdr patch performance
is excluded from these claims and from the shipped implementation.

## Reproduce

Use the Node version required by `package.json`, the Rust toolchain pinned
in `.github/workflows/ci.yml`, an installed official Herdr release matching
`electron/herdr-contract.cjs`, and a working Electron environment. All test
profiles, checkout directories, sockets and live test sessions are isolated.
No command below packages or publishes the application.

```sh
npm run ci
npm run test:desktop
npm run test:herdr
npm run test:stream
npm run test:unicode
npm run test:terminal
node scripts/herdr-ui-evidence.mjs
node scripts/terminal-flow-load.cjs
node scripts/herdr-app-benchmark.mjs . artifacts/herdr-app-after.json
```

For Linux without a display, use `xvfb-run -a -s '-screen 0 1600x1200x24'`
around Electron drivers. Set `ELECTRON_OVERRIDE_DIST_PATH` only when the
installed Electron distribution lives elsewhere. Every app driver supplies
`SUSHIAI_TEST_WINDOW=hidden`.

Run the app benchmark against a built checkout of `113f576` for the baseline.
Its default idle phase is 120 seconds. For real SSH, set
`SUSHIAI_HERDR_APP_TRANSPORT=ssh`,
`SUSHIAI_HERDR_BENCH_SSH_PORT` and `SUSHIAI_HERDR_BENCH_SSH_KEY` to an
isolated sshd fixture. `scripts/herdr-benchmark.cjs` supplies the separate
runtime and lifecycle checks; `scripts/herdr-install-verification.cjs` checks
fresh installation and repeat verification on local and SSH hosts.

Raw app, lifecycle and compatibility records are also checked in under
`docs/verification/herdr/`. App screenshots and repeat records are retained under
`artifacts/herdr-stability/`; terminal samples, summary and screenshots under
`artifacts/herdr-terminal-flow/`; real UI migration evidence in
`artifacts/herdr-ui-evidence.json` and `artifacts/herdr-migration-layout.png`.
The checked-in `docs/measurements/herdr-stability.json` retains a compact
measurement record and SHA-256 hashes for the raw evidence.

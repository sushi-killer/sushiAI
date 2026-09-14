---
name: sushiai-testing
description: "Choose proportional proof for a sushiAI change - which npm scripts, targeted node --test suites, desktop smoke or manual steps a diff needs - and read the results honestly. Use before claiming any change works, when a check fails, or when briefing functional-qa."
---

# sushiAI testing

Prove the changed contract with the smallest meaningful check, then run the
required gate once. Broaden only for a new change, a failure or an open risk.

## Select the proof

| Change                                                  | Proof                                                                                   |
| ------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Any code change (required gate before commit)           | `npm run ci` - build, full `node --test`, conventions, eslint, prettier, `git diff --check` |
| Manifest, registry, routes, IPC surface                 | `npm run test:contracts`                                                                |
| Extension loading or surface state                      | `npm run test:extensions`                                                               |
| Layout, navigation, workspace or chat state             | `npm run test:core`                                                                     |
| `src/app`, `src/extensions` or `electron/`              | `npm run build`, then `npm run test:desktop`, and `$ui-evidence` when a screen changed  |
| Terminal rendering or unicode                           | `npm run test:terminal`, `npm run test:unicode`                                         |
| Herdr stream or integration                             | `npm run test:stream`, `npm run test:herdr`                                             |
| Startup or render cost                                  | `npm run perf`                                                                          |
| Docs, skills or agent files only                        | `npm run check:conventions` and `git diff --check`                                      |

Run a single file with `node --test tests/<name>.test.cjs`.

## Read results honestly

- The exit code is the verdict. Record `echo $?` or redirect to a file and read
  it back; a wrapper has printed a success line for a run that exited 1.
- Never verify through `npx`; use the npm script or `./node_modules/.bin/<tool>`.
- Tests are `.cjs` files importing `.ts` through Node's type stripping: nothing
  type-checks at test time, so an assertion must exercise behaviour. A test
  that only matches a string in the source proves nothing.
- `npm run test:desktop` tests `dist/`; it refuses a `dist` older than `src`.
  Build first.
- `tsc` covers only `src/`; `electron/` and `tests/` rely on ESLint.

## Writing tests

- A regression test fails on the original defect before the fix.
- Synthetic data only: `192.0.2.0/24` addresses, invented hosts and names. CI
  rejects private network addresses in `src`, `electron`, `tests`, `scripts`.
  When a test needs a private address as input, build it at runtime.
- Temporary trees go under `os.tmpdir()` and are removed; require `fs`, `os`,
  `path` inside the test that uses them.
- Do not hide a failure with retries, longer timeouts, weaker assertions or a
  refreshed baseline.

## When a check fails

Separate a product defect, a test defect and an environment problem before
changing anything. Fix failures the diff caused. Report a proven unrelated
failure with its evidence instead of widening the task.

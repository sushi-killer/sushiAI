---
name: ui-evidence
description: "Produce measured, looked-at evidence of the sushiAI UI: attach to the owner's running `npm run dev` window (first choice), or drive the built Electron app with Playwright against a throwaway profile; measure geometry, capture screenshots and read them. Use whenever a change touches what a screen shows, including a second SSH host, before claiming it looks right."
---

# UI evidence

Green tests do not prove a screen looks right. Measure it, photograph it, and
open every image you produced before you describe it.

## First choice: attach to the owner's running dev app

When the owner runs `npm run dev`, look at that window with its real data
instead of a throwaway profile; problems of the live app show only there.
`scripts/dev.mjs` starts Electron with `--remote-debugging-port=0` bound to
`127.0.0.1` (dev only, never in the packaged app; `check-conventions` enforces
it) and writes `~/.sushiai-dev/devtools.json` (`{port, wsEndpoint, pid}`, mode
0600). The path is fixed, whatever `SUSHIAI_HOME` the dev run uses. The file is
deleted when Electron exits. A missing or stale file means no dev app is
running: fall back to the steps below.

**Security.** While dev runs, any local process can control the dev window
over that port. The 0600 mode only limits who can discover the port; it is not
protection. The feature is on by default in dev; start dev with
`SUSHIAI_DEV_ATTACH=0` to turn it off.

```
node .agents/skills/ui-evidence/scripts/attach.mjs shot <out.png> [--selector css] [--full]
node .agents/skills/ui-evidence/scripts/attach.mjs measure <css...>   # JSON boxes, scroll sizes, computed styles
node .agents/skills/ui-evidence/scripts/attach.mjs eval <expr>        # read-only expressions only
node .agents/skills/ui-evidence/scripts/attach.mjs click <css>
node .agents/skills/ui-evidence/scripts/attach.mjs type <css> <text>
node .agents/skills/ui-evidence/scripts/attach.mjs key <combo>
```

- It picks the main app window (the page at the dev URL root; not DevTools, not the mascot) and checks the port answers as the file says. It never resizes
  or moves it unless you pass `--viewport WxH`. Resize only on request, and say
  so: it changes the owner's window until the next attach resets it.
- `type` appends at the end of the field, typing like a person; it does not clear it.
- `shot` and `measure` are free. `click`, `type` and `key` act on the owner's
  live data: ask before any that sends, deletes or archives. `eval` must not
  mutate state. There is no route API, so there is no `goto`.
- Open every PNG with the Read tool, as in step 5 below.

### As the ui-match `app.cdp` source

The kit's `ui-match` tool attaches to a running app through `app.cdp`
(an http endpoint). Read the port from `devtools.json` and put it in the spec:

```json
{ "app": { "cdp": "http://127.0.0.1:<port from devtools.json>" } }
```

For example `jq .port ~/.sushiai-dev/devtools.json` gives the number. The
port changes on every dev start, so write the spec after the app is up. An
attached app is not reloaded: write the state steps to reach each state from
the previous one, and ask before steps that change data.

## Run (throwaway profile)

1. **Build.** `npm run build` - the driver launches `dist/`, not the sources.
   **Done when:** the build exits 0 after your last edit.
2. **Copy the driver.** `cp .agents/skills/ui-evidence/scripts/driver-template.mjs artifacts/.driver.mjs`.
   It must live inside the repo (`artifacts/` is gitignored) so `playwright`
   resolves. Replace its `STEPS` block with the flow under test.
   **Done when:** the driver selects elements by role or stable class, not by
   position.
3. **Second host, if the change is about remotes.** Set `SUSHIAI_EVIDENCE_SSH`
   in the shell for this run only. The driver adds the host through Settings -> Connections
   in the throwaway profile and removes it afterwards. Never write a real host
   into a committed file.
4. **Run and measure.** `node artifacts/.driver.mjs`. Measure what the request
   is about:
   - text-to-neighbour distance from the text ink (`document.createRange()`
     over the text node), never from a box that `flex: 1` may have stretched;
   - every label's rendered width is non-zero (a squeezed label still "exists");
   - `scrollWidth - clientWidth` of scroll containers (horizontal overflow);
   - computed styles when two elements must match exactly;
   - accessible names through `getByRole`.
     **Done when:** each claim in your reply is a number from this run.
5. **Capture and look.** Screenshot the full window and a crop of the area in
   question. Crops narrower than ~400px are unreadable; widen the clip rather
   than upscaling. Open every PNG with the Read tool and say what you see.
   When `elementFromPoint` shows something covering the element (a
   `.panel-error` overlay in a throwaway profile, a toast), name it instead of
   calling the element broken.
   **Done when:** every screenshot you cite was opened in this session.
6. **Clean up.** Delete `artifacts/.driver.mjs`; keep only the PNGs you cite.

## Orchestrator panel task detail

To screenshot one orchestrator task's detail view, skip the steps above and run
`node .agents/skills/ui-evidence/scripts/orchestrator-panel.mjs <seed.json> <task title>`
(build prerequisites still apply). `<seed.json>` is a JSON array of task
objects, or `{tasks, proposals, notes, settings}` to also seed evolution proposals,
repo notes and daemon settings. Optional `settings` overrides are merged over the
daemon's `settings.get` result and saved through `settings.set`, retrying until
the daemon answers, after the extension is enabled and before the panel mounts;
the full settings retain the required routes and tiers. Each
proposal is completed with an id (unless it has one), `repo` = this repo and
`createdAt`, then written to `<home>/orchestrator/evolution/proposals/<id>.json`, where
the orchestrator's store reads it. Each note (`text`, optional `id`, `source` - default `owner`, or `proposal:<id>` - and `createdAt`) is written under this repo's root in `<home>/orchestrator/repo-notes.json`, the one file keyed by repo that the orchestrator reads. Task objects - only `title` is required, `status`/`tier`/`decisions`/`criteria`/
`attempts` (with `costUsd`) all default; `status` must be done, failed,
stopped or waiting. Optional `pausedInHours` (a number) becomes
`pausedUntil = now + pausedInHours * 3600000` and is omitted from the stored task,
so a usage-limit pause stays relative to each run rather than a fixed date.
`evidence: {"<attempt n>": [image paths]}` copies screenshots into that attempt's evidence dir. Give a seed a `key` to let others name it in `parent`,
`dependsOn` (a task graph), `followUps` or `followUpOf`. A seed may include
`git: {base, commits}` to create a temporary Git repo, its base branch, and
that many patch-unique commits on the task branch; all seeded tasks then use
that repo, and the fixture becomes the saved workspace cwd. The driver expands
Details and waits for an unlanded label when the selected seed includes one.
The tasks are written into the throwaway daemon home (with the module's
enabled flag) before the app starts its sushiai daemon there - no harness run, no API key. It opens the
panel, picks the task by title, saves
`artifacts/orchestrator-{window,detail,home}.png` (`home` is the Home view before a task is picked; `detail` crops the task view `.td`), plus `artifacts/orchestrator-proposals.png` (a crop of the first proposal card on the Improvements view, only when proposals were seeded; `report.screenshots.proposals`), plus `artifacts/orchestrator-improvements.png` (the whole Improvements view, when proposals or notes were seeded) and `artifacts/orchestrator-repo-notes.png` (a crop of the notes list on that view, only when notes were seeded; `report.screenshots.notes`), prints a JSON report, and
exits non-zero with `error` set on any failure, including an unmatched title.

## Orchestrator blueprint

To photograph the orchestrator's pipeline diagram (a running task, a failed
task and the rail's Pipeline view with one stage's inspector open), at a narrow
and a wide panel, run
`npm run build && cargo build -p sushiai && node .agents/skills/ui-evidence/scripts/orchestrator-blueprint.mjs`
(no arguments). It uses a throwaway profile (`HOME`, `CODEX_HOME`,
`SUSHIAI_HOME=<profile>/sushiai`, `SUSHIAI_TEST_WINDOW=hidden`) with the
Orchestrator extension enabled, and seeds two tasks into
`<profile>/sushiai/orchestrator/tasks/<id>/task.json` with `repo` = the current
directory (`task.list` filters on equality):

- a failed task whose implement attempt failed `verify` (with the failing
  command's outcome, a cost and timing), so the Checks stage is danger;
- a queued task with a `backlog` entry (so the daemon does not resume it on
  start), empty `verify` and `finalVerify`, and a real git worktree and branch
  `task/<id>` created by the driver (starting a task whose branch is missing
  fails). It is listed in the Plan view, not the rail. The driver picks the
  Fast preset on it (the fake harness never answers a brief check), clicks
  Start, waits for the running `Implement` stage, and clicks `Stop task`
  before the app closes.

No real harness runs: `<profile>/bin/claude` and `<profile>/bin/codex` print
the init line the harness parser reads and then `exec sleep 600`, and
`<profile>/bin` is first on the `PATH` given to the app (orchd runs a bare
`claude` / `codex` from `PATH`). Rail rows and views are picked at the wide
size first, because below 640 px the panel's container query hides the rail.
The window is resized until the Orchestrator panel measures about 1100 px
(wide) and about 520 px (narrow); the measured widths are in the report.

It saves `artifacts/blueprint-running-{narrow,wide}.png` and
`artifacts/blueprint-failed-{narrow,wide}.png` (crops of the task view `.td`),
and `artifacts/blueprint-repo-inspector-{narrow,wide}.png` (the Pipeline view
with the Advisor inspector). In `finally` it stops the daemon, then removes
only its own worktree and branch (`git worktree remove --force`,
`git branch -D task/<id>`, `git worktree prune`). It prints a JSON report
`{screenshots, widths, pageErrors, error?}` and exits non-zero on a failed step
or a page error.

## Worktree grouping and cleanup

For workspace grouping, flat-list stability and merged/closed PR cleanup
defaults, run `npm run build && node .agents/skills/ui-evidence/scripts/worktrees.mjs`
from the repository root. The driver creates a disposable local repository
and linked worktree, uses a fake `gh` response, and gives sushiAI a
daemon home of its own. It measures the workspace row count, label width and overflow,
samples the flat list for two seconds, then opens the close dialog without
submitting it. It saves full-window and focused screenshots under `artifacts/`
and exits non-zero on renderer errors or a failed step.

## Orchestrator task notices

To photograph the orchestrator notices on the desktop mascot (needs input, done,
failed), the quick answer and what "Open" does, run
`npm run build && npm run build:daemon && node .agents/skills/ui-evidence/scripts/orchestrator-notices.mjs`
(no arguments). It seeds one landed done task with cost, one failed task whose
last attempt failed with kind `verify`, and one waiting task with a question
and the options Delete it / Keep behind a flag / Stop, opens a Local Evidence
workspace with no Orchestrator panel, and pushes each task through the real
main-process path (`orchestratorNotice` -> the mascot queue) using the
`SUSHIAI_TEST_MASCOT=1` seam in `electron/main.cjs` (a main-process global; no
renderer-reachable channel injects notices). The mascot is a separate Electron
window, found with Playwright `app.windows()` by its `mascot.html` URL. It
saves `artifacts/mascot-{done,failed,input,answered}.png` (the answered shot
clicks the Stop option - the orchestrator treats `stop` as stop, so no harness runs), and
after clicking Open on the mascot's input and done bubbles
`artifacts/orchestrator-open-{input,done}.png`. `artifacts/mascot-report.json`
holds the mascot bounds vs the primary work area, `isAlwaysOnTop`,
`isVisibleOnAllWorkspaces`, the focused window before and after the mascot
shows, its visibility once the queue is empty, and whether `.td-question` /
`.td-report` intersect the viewport with the `.selected` task row; the script
exits non-zero with `error` set on any failure.

The needs-input bubble grows with its question up to 70% of the work area
height, then its body/options area scrolls. Two more waiting seeds cover that:
`artifacts/mascot-input.png` (a, the short question),
`artifacts/mascot-input-long.png` (b, a two-line title, a four-sentence body
and three long options) and `artifacts/mascot-input-cap.png` (c, a body of
about 1200 characters and four options of about 40 words). They are pushed
after the short shot and dismissed before the rest of the run. For each,
`layout.{short,long,cap}` in `mascot-report.json` has `windowHeight` vs `cap`
(`floor(0.7 * workArea.height)`), whether the title, Answer field, Open and
Dismiss lie fully inside the mascot viewport, `titleLines`, each option's
`innerText` and accessible name (they must equal the seeded option), and the
scroll area's `scrollHeight`/`clientHeight`. The script fails unless (a) and
(b) are not scrolled, (c) is scrolled at exactly the cap, and every window
stays anchored bottom-right.

## Drivers

Run each after `npm run build`, from the repo root. Open every PNG they write.

| Driver                                 | Covers                                                                 |
| -------------------------------------- | ---------------------------------------------------------------------- |
| `driver-template.mjs`                  | any flow: copy it, replace `STEPS`                                     |
| `routines.mjs`                         | Routines page R1-R11 with a stubbed module, widths 1440 to the minimum |
| `builder.mjs`                          | routine builder chat, see below                                        |
| `orchestrator-panel.mjs`               | one orchestrator task detail from a seed                               |
| `orchestrator-blueprint.mjs`           | pipeline diagram, narrow and wide                                      |
| `orchestrator-notices.mjs`             | mascot notices and the quick answer                                    |
| `worktrees.mjs`                        | workspace grouping and worktree cleanup                                |
| `mascot.mjs`, `mascot-core-update.mjs` | the desktop mascot                                                     |

## Routine builder chat

`node .agents/skills/ui-evidence/scripts/builder.mjs` (no arguments) feeds the
real builder path with a fake model: a `claude` and a `codex` in a temp dir at
the front of `PATH` print the turn the driver queued, and the throwaway daemon
home carries `modules/routines.enabled`. `HOME`, `SUSHIAI_HOME` and the app data dir are in the
throwaway profile `/tmp/sbld-*`. Routine list, show and grant are stubbed. It saves
`artifacts/routines-mvp/screens-v4/<state>-<1440|900|600>.png` for: `a-ask`
(owner card with a choice, a choice with Other, a text and a multi question, one
action), `a2-filled`, `b-needs-you` (run-result JSON shown readable),
`c-long` (long Russian message with a list), `d-access-sheet` and
`d-access-notice` (built routine with ungranted access), `e-failed`,
`f-no-sushiai` (codex, "sushiai is not available"). The JSON report lists
sideways overflow (exit 1) and interactive elements under 24 px (`smallTargets`).
It also writes `artifacts/builder-<state>.measure.json` at 1440.

## Boundaries

- Drivers launch with `SUSHIAI_TEST_WINDOW=hidden`; a visible window is an
  explicit per-driver opt-out (`"visible"` plus a comment saying why).
- The driver launches its own Electron with a temporary `BRIDGE_DATA_DIR`.
  Only `attach.mjs` touches the owner's running dev app, and only through its
  devtools endpoint. Never restart or kill the owner's app or dev server.
- Keep the template's `SUSHIAI_HOME=<profile>/sushiai`: the app then starts a
  daemon in the throwaway profile. Without it the test app does not start a
  daemon, and pointing it at `~/.sushiai` would show the owner's real sessions.
- The daemon outlives the app by design: end the run with `stopDaemon` from
  `scripts/lib/daemon-binary.mjs` (the drivers do), and never stop a daemon
  this run did not start.
- Visual taste is `design-critic`'s call; this skill reports measurements and
  images.

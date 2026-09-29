---
name: ui-evidence
description: "Produce measured, looked-at evidence of the built sushiAI UI: drive the real Electron app with Playwright against a throwaway profile, measure geometry, capture screenshots and read them. Use whenever a change touches what a screen shows, including a second SSH host, before claiming it looks right."
---

# UI evidence

Green tests do not prove a screen looks right. Measure it, photograph it, and
open every image you produced before you describe it.

## Run

1. **Build.** `npm run build` - the driver launches `dist/`, not the sources.
   **Done when:** the build exits 0 after your last edit.
2. **Copy the driver.** `cp .agents/skills/ui-evidence/scripts/driver-template.mjs artifacts/.driver.mjs`.
   It must live inside the repo (`artifacts/` is gitignored) so `playwright`
   resolves. Replace its `STEPS` block with the flow under test.
   **Done when:** the driver selects elements by role or stable class, not by
   position.
3. **Second host, if the change is about remotes.** Set `SUSHIAI_EVIDENCE_SSH`
   (and `SUSHIAI_EVIDENCE_SOCKET` when the Herdr socket differs) in the shell
   for this run only. The driver adds the host through Settings -> Connections
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

To screenshot one orchd task's detail view, skip the steps above and run
`node .agents/skills/ui-evidence/scripts/orchestrator-panel.mjs <seed.json> <task title>`
(build prerequisites still apply). `<seed.json>` is a JSON array of task
objects, or `{tasks, proposals}` to also seed evolution proposals: each
proposal is completed with an id (unless it has one), `repo` = this repo and
`createdAt`, then written to `<dataDir>/evolution/proposals/<id>.json`, where
orchd's store reads it. Task objects - only `title` is required, `status`/`tier`/`decisions`/`criteria`/
`attempts` (with `costUsd`) all default; `status` must be done, failed,
stopped or waiting. `evidence: {"<attempt n>": [image paths]}` copies screenshots into that attempt's evidence dir. Give a seed a `key` to let others name it in `parent`
or `dependsOn` (a task graph). The tasks are written into the throwaway profile before
the app starts its own orchd there - no harness run, no API key. It opens the
panel, picks the task by title, saves
`artifacts/orchestrator-{window,detail,home}.png` (`home` is the orchestrator page before a task is picked), plus `artifacts/orchestrator-proposals.png` (a crop of the PROPOSALS list, only when proposals were seeded; `report.screenshots.proposals`), prints a JSON report, and
exits non-zero with `error` set on any failure, including an unmatched title.

## Boundaries

- The driver launches its own Electron with a temporary `BRIDGE_DATA_DIR`.
  Never attach to, restart or kill the owner's running app or dev server.
- Keep the template's `HERDR_SOCKET_PATH` pointing at a missing file: unset,
  the app connects to the owner's real Herdr and screenshots show their
  projects.
- A temporary `BRIDGE_DATA_DIR` does not isolate Herdr: the daemon and its
  workspaces are machine-wide, and New workspace defaults to the Herdr backend
  when it is connected. Create evidence workspaces with the Local backend only,
  and never close a Herdr workspace this run did not create.
- Visual taste is `design-critic`'s call; this skill reports measurements and
  images.

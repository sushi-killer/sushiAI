---
name: figma-to-code
description: "Turn a Figma concept frame into exact code metrics and check the built screen against it: export a node dump to a snapshot, generate token-only CSS, measure the running app and compare, with a pass/fail table. Use whenever a screen must match a Figma frame 1:1, when the owner says a screen must look like the concept, or before design-critic reviews a converted screen."
---

# Figma to code

A PNG is a picture, not a spec. This loop reads the numbers out of Figma and
checks the built app against them, so "matches the concept" is an exit code.

The figma-console MCP is available only to the lead session. The lead extracts
dumps; an implementer works from the saved files. Everything under
`concepts/figma/export/` is gitignored and holds real concept names: never copy
its strings into a committed file, test or doc.

## Loop

1. **Extract (lead).** For each frame call `figma_get_component_for_development_deep`
   with the frame node id and save the tool-result JSON as
   `concepts/figma/export/raw/R<n>.json`.
   **Done when:** the file parses and its top node is the 1380x864 window frame.
2. **Snapshot.** `node scripts/figma-export.mjs concepts/figma/export/raw/R1.json`
   writes `concepts/figma/export/snapshots/R1.json`: layer tree with boxes
   relative to the content root, layout, radius, stroke, fill and text metrics.
   The root is found by stripping the shell (title bar, sidebar); pass
   `--root <layer name>` when it picks wrong.
   **Done when:** the printed root is the area under test and every
   `UNMAPPED`/`UNBOUND`/`DRIFT` line is either understood or handed to the owner
   as a vocabulary item (a Figma variable with no token in `src/styles/tokens.css`).
3. **Codegen or hand-apply.** `node scripts/figma-codegen.mjs <snapshot>` writes
   token-only CSS classes and a role to class map under
   `concepts/figma/export/generated/`. It is a helper: copy the values into the
   real component styles, in `src/styles/components/`, reusing existing classes
   (`$sushiai-ui`). It never becomes a runtime dependency.
   **Done when:** the component CSS spends tokens only and no `fx-` class ships.
4. **Parity.** Build (`npm run build`), copy the ui-evidence driver, and add the
   three lines from `.agents/skills/ui-evidence/scripts/measure.mjs` (or run the
   template with `--measure`) to write `artifacts/<name>.measure.json`. Then
   `node scripts/figma-parity.mjs <snapshot> artifacts/<name>.measure.json`.
   Tolerances: 2px position, 3% text width, exact font size, weight and colour
   token. Text is the anchor; an unmatched Figma text is a FAIL, an extra app
   text is listed only.
   **Done when:** the exit code is 0, or every FAIL is fixed in the code or
   recorded as a decision for the owner (Figma wrong vs app wrong).
5. **Look.** Screenshot the screen and give it with the parity table to
   `design-critic`. Numbers do not replace looking.

## Rules

- The fix goes into the app code or the Figma file, never into the tolerance.
- A Figma colour with no token is a question for the owner: add the token to
  `src/styles/tokens.css` for everyone, or change the design.
- Positions come from auto-layout; a layer inside a frame without auto-layout
  has no position in the dump and is marked `approx`. Do not trust its x/y.
- Extract only through the Desktop Bridge tools (`figma_get_component_for_development_deep`,
  `figma_take_screenshot`). `figma_get_file_data` is a REST tool and fails without a
  `FIGMA_ACCESS_TOKEN`; that is not a bridge failure. A dump too big for the reply is saved
  to a file: copy it from the tool-result path into `concepts/figma/export/raw/`.
- A dialog or overlay frame needs its own root: find the layer name (for example `dialog`)
  in the dump and pass `--root <name>`, or the exporter picks the page behind it.

---
name: sushiai-ui
description: "The sushiAI design code: tokens, type scale, the component catalogue and the recipes for new windows, taken from the shipped screens. Use before writing TSX or CSS for any change that adds or alters a screen, dialog, window, panel, menu, popover, list row, settings section or component in src/, and whenever Figma is unavailable. It replaces outside design skills for this repo."
---

# sushiAI UI

The design code of the app as it ships today, so a new screen looks like it
was always there. Figma is the source when the owner provides a concept; when
there is none, this skill is the concept. It replaces every outside design
skill (frontend-design, ui-ux-pro-max, design, ui-styling and the like) for
sushiAI: do not load them, their taste is not this app's.

Every value here was read from the code. If the code and this file disagree,
the code wins and this file is the one to fix.

## The style in one paragraph

Dark, dense, quiet tool chrome. Near-black surfaces separated by one-shade
steps and hairline borders, not by shadows. One sage accent (`--accent`) for
the single primary action and "on" states; status is a 6px tone dot or a
16px tag, never a coloured card. Text is the system sans at 10-13px with
fixed pixel line-heights; only dialog titles go to 18-20px. Weights are 400,
500 and 600, nothing else. Uppercase 10px eyebrows and group labels carry
structure. Motion is almost absent. Plugins render inside the app's own
window and never take over the window frame or the left navigation.

It is not: a marketing page, a dashboard of cards, a gradient, a hero, a
centred stack of big inputs, emoji, coloured panels, or copy that explains
itself.

## Rules

1. **Reuse, then extend, never fork.** Find the nearest existing pattern in
   `references/components.md` and open its canonical file before writing a
   line of CSS. Use its classes. If it lacks a variant, add a modifier class
   next to it. If a pattern scoped to one area (`pk-`, `np-`, `pd-`) is
   needed in a second area, promote it to a shared `ui-` primitive in
   `src/orchestrator/ui/` in the same change and move the old callers - do
   not copy it under a new prefix.
2. **Spend tokens.** Colours, radii, gaps and type come from
   `src/styles/tokens.css` (`references/tokens.md`). CI rejects a raw hex in
   `src/styles/**/*.css` outside `src/styles.css` and `tokens.css`. Never use
   a `var(--x)` that tokens.css does not define - an undefined variable fails
   silently. A colour the app does not have is not added in a feature change.
3. **New CSS goes in `src/styles/components/<area>.css`**, imported once in
   `src/main.tsx`, with a header comment naming the screen. That folder is
   what the CI colour check covers; `src/styles.css` is legacy and only
   shrinks.
4. **Scope inside dialogs.** The legacy `.modal form`, `.modal label`,
   `.modal input`, `.modal h2` rules restyle everything in a dialog. Write
   two-class selectors (`.pd .pd-input`, `.np .np-head h2`) so yours win; see
   the specificity section of `references/recipes.md`.
5. **Icons are lucide-react, sized by role** (12 inline, 14 in a labelled
   button, 15 in a nav rail, 17 in a picker row or the dialog close X).
   Decorative icons get `aria-hidden`. Agents use their own marks
   (`Icon` in `src/PanelIcon.tsx`), never an emoji.
6. **Provenance and status sit at the name**: a small icon or `Tag` right
   after it, the full words in `title`. No heading rows per category, no
   repeated status pills down a list.
7. **Copy is short and states consequences.** Sentence case, English, no
   exclamation marks. A hint says what happens ("Tasks branch from here and
   land back into it."), not what the control is. `·` separates facts, `…`
   ends a label that opens a confirmation, `→` names a path through the UI.
8. **Accessible by construction**: real `button`/`input`/`select`, roles on
   custom widgets (`tablist`, `radiogroup`, `menu`, `menuitemradio` with
   `aria-checked`), `aria-label` on icon-only buttons, a visible focus ring,
   `role="alert"` on errors. Tests and the evidence driver select by role.

## Catalogue at a glance

| Need                      | Use                                   | Canonical file                                                |
| ------------------------- | ------------------------------------- | ------------------------------------------------------------- |
| Dialog shell              | `DialogHost` + `.modal.<name>`        | `src/dialogs/DialogHost.tsx`                                  |
| Two-pane settings window  | rail + main (`settings-*` / `pd-*`)   | `src/app/SettingsDialog.tsx`, `src/ProjectSettingsDialog.tsx` |
| Compact dialog (640px)    | `pk-head` / `np-head`, footer         | `src/app/PanelPickerDialog.tsx`, `src/WorkspaceDialog.tsx`    |
| Page with title + scroll  | `ProjectPage` (`pd-title`, `pd-body`) | `src/ProjectPage.tsx`                                         |
| Labelled setting row      | `Field` (`pd-field`)                  | `src/ProjectGeneralTab.tsx`                                   |
| Choice list row           | `pk-row` + `pk-row-main` + `pk-text`  | `src/app/PanelPickerDialog.tsx`                               |
| Dropdown / account menu   | `picker-account-menu` + `pk-menu-*`   | `src/app/PanelPickerDialog.tsx`                               |
| Table of records          | `pd-table` + `pd-row` (+ `.head`)     | `src/ProjectEnvironmentTab.tsx`                               |
| Buttons                   | `ui-button primary/secondary/ghost`   | `src/orchestrator/ui/ui.css`                                  |
| Switch, tag, chip, banner | `Toggle`, `Tag`, `Chip`, `Banner`     | `src/orchestrator/ui/`                                        |
| Status dot                | `ui-dot ui-tone-*`                    | `src/orchestrator/ui/ui.css`                                  |
| Key hint                  | `kbd.pk-kbd`                          | `src/styles/components/picker.css`                            |
| Error / empty             | `pd-alert`, `pd-empty`, `Empty`       | `src/styles/components/project.css`, `src/app/Empty.tsx`      |

Full entries with sizes and snippets: `references/components.md`. Recipes for
a new settings section, dialog, project tab, menu and list row:
`references/recipes.md`.

## Concept loop when there is no Figma

1. **Describe the screen in words against the catalogue** before coding:
   which shell, which rows, which controls, the one primary action, the empty
   and error states, and the nearest existing screen it must sit beside.
   **Done when:** every element in the description names a catalogue entry
   or a deliberate, stated extension of one.
2. **Build it from those components**, tokens only, CSS in its area file.
3. **`npm run build`**, then photograph it with `$ui-evidence` (driver in
   `artifacts/`, hidden window, throwaway profile). Capture the new screen
   and the nearest existing screen at the same window size.
4. **Compare side by side**: left edges and baselines line up, row heights
   and gaps match the neighbour's rhythm, the same text role has the same
   size and weight, one accent at most. Measure, don't eyeball: text ink via
   `Range`, computed styles for anything that must match. Fidelity
   tolerances as in `$sushiai-task`: text width within 3%, shifts within
   2px.
   **Done when:** every difference is either fixed or written down as
   intended.
5. **`design-critic` judges** the PNGs in fresh context. Fix and re-shoot;
   one full pass and one delta pass.

Green tests never prove a screen looks right. Open every PNG you cite.

## Slop tells to refuse

Gradients or glows as decoration (the ones that exist carry meaning: the
`pd-body` scroll-fade mask, the hatched waiting stage, a loading shimmer, an
image's transparency checkerboard); emoji as icons; a 25px+ heading inside a tool pane (the legacy
`.modal h2` is 25px - override it); a card inside a card; a centred column of
stacked full-width inputs where the app uses label-left rows; the same status
pill repeated on every row; a new colour, shade or shadow; mixed icon sizes in
one row; buttons of different heights side by side without reason; copy that
narrates ("Here you can manage..."), filler subtitles, title case
everywhere; a third copy of a menu, input or segmented control. More, with
the real counter-examples: `references/recipes.md`.

## Before you hand it over

- [ ] Nearest existing pattern named and reused; no forked class family.
- [ ] No raw colour in new CSS; every `var(--x)` exists in tokens.css.
- [ ] New CSS in `src/styles/components/<area>.css`, imported in `src/main.tsx`.
- [ ] Sizes come from the scale in `references/tokens.md`; weights 400/500/600.
- [ ] Icons lucide-react at the role size, `aria-hidden` when decorative.
- [ ] Empty, loading and error states exist and use the catalogue classes.
- [ ] Keyboard: Escape closes the innermost layer only; focus ring visible.
- [ ] Nothing renders outside the app's own content area or over its nav.
- [ ] Built, photographed with `$ui-evidence`, compared against its neighbour,
      judged by `design-critic`.
- [ ] `./node_modules/.bin/prettier --write` on touched files; `npm run ci`.

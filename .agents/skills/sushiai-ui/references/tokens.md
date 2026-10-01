# Tokens and scale

Source: `src/styles/tokens.css` (the vocabulary) and what the converted
stylesheets actually spend: `src/styles/components/*.css`,
`src/orchestrator/**/*.css`, `src/app/settings-dialog.css`. Values are listed
so you can recognise them in a computed style or a screenshot. Never type one;
write the variable.

## Colour tokens

### Surfaces - depth by one-shade steps, darkest at the back

| Token           | Value               | Use                                                                          |
| --------------- | ------------------- | ---------------------------------------------------------------------------- |
| `--bg-app`      | `#0b0b0b`           | window, Settings main column, inputs and segmented-control wells             |
| `--bg-sunken`   | `#101210`           | dialog side rails (`settings-nav`, `pd-rail`)                                |
| `--bg-dialog`   | `#131313`           | compact and project dialogs (`command-modal`, `np-dialog`, `project-dialog`) |
| `--bg-raised`   | `#171717`           | table head row, cards, secondary button                                      |
| `--bg-hover`    | `#1b1d1b`           | row and nav hover                                                            |
| `--bg-elevated` | `#1b1b1b`           | popovers and menus, the in-dialog confirm box                                |
| `--bg-selected` | `#1d241d`           | selected chip, host, menu item, focused row (green-tinted)                   |
| `--bg-control`  | `#232323`           | chips, host buttons, selected nav item                                       |
| `--bg-count`    | `#262626`           | count badges, kbd, toggle track (off)                                        |
| `--scrim`       | `rgb(0 0 0 / 0.55)` | dialog backdrop, the in-dialog confirm overlay                               |

### Lines

| Token             | Value     | Use                                                    |
| ----------------- | --------- | ------------------------------------------------------ |
| `--line`          | `#303030` | default border: inputs, chips, tables, dialog frame    |
| `--line-pane`     | `#2a2a2a` | pane and rail separators, attention cards              |
| `--line-divider`  | `#202020` | row separators inside a list, footer rules, menu rules |
| `--line-strong`   | `#4a554b` | secondary button border, popover border, checkbox box  |
| `--line-selected` | `#71866e` | border of a selected chip, host, row                   |

### Text

| Token          | Value     | Use                                              |
| -------------- | --------- | ------------------------------------------------ |
| `--text`       | `#e7e8e0` | titles, values, selected labels                  |
| `--text-muted` | `#8f948d` | hints, subtitles, unselected chips, icons        |
| `--text-faint` | `#6c726d` | eyebrows, group labels, meta lines, placeholders |
| `--text-nav`   | `#929292` | unselected nav rail items, count text            |

### Accent, state, tones

| Token                                                     | Use                                                                                     |
| --------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| `--accent` / `--accent-ink`                               | the one primary button, toggle on, checked box, lit step                                |
| `--accent-sage/-blue/-amber/-violet`                      | the only accents an extension may name; also agent glyphs (Claude amber, Gemini violet) |
| `--danger`, `--danger-bg`, `--danger-line`                | error text, destructive hover, error well                                               |
| `--focus` (`#7797d7`)                                     | focus ring and focused input border                                                     |
| `--tone-{ok,info,warning,danger,neutral,muted}` and `-bg` | meaning of a value; spent through `.ui-tone-*`                                          |

`.ui-tone-ok|warning|danger|info|neutral` (in `src/orchestrator/ui/ui.css`)
sets `--ui-tone` and `--ui-tone-bg` on an element; `.ui-dot`, `.ui-tag`,
`.ui-banner`, `.pd-dot` read them. Use a tone class, never a tone colour
directly on a new component. The `Tone` type is
`"ok" | "warning" | "danger" | "info" | "neutral"` (`src/orchestrator/helpers.ts`).

`--shadow-popover` (`0 8px 30px #0008`) is the only shadow a new component
uses, and only on something that floats (menu, popover).

## Legacy raw colour - do not copy

`src/styles.css` (about 5,600 lines) still holds about 730 raw hex values and
is exempt from the CI check; so are `src/agents/agents.css` (about 110) and
`src/ChatView.css` (about 50), which sit outside `src/styles/`. Examples you
will meet: `.primary` (`#e0e0dc` light button), `.secondary` (`#272727`),
`.inline-error` (`#3a2521`), `.modal` (`#161616`, border `#3c3c3c`),
`.status-dot.green` (`#7cdb9b`), `.muted` (`#777`). They predate the tokens.
When you touch such a rule for your feature, convert the lines you touch to
tokens; never paste one into a new file.

## Type

Family: `--font-sans` (system UI) everywhere; `--font-mono` for paths, keys,
hosts, commands, env names (`.pd-mono`, `.pd-remote`, `.pd-server-line`).

| Role                                             | Size             | Weight  | Line-height | Example selector                            |
| ------------------------------------------------ | ---------------- | ------- | ----------- | ------------------------------------------- |
| Page title in a two-pane dialog                  | 20px             | 600     | 24px        | `.settings-title h2`, `.pd-title h2`        |
| Compact dialog title                             | 18px             | 600     | 22px        | `.pk-head h2`, `.np-head h2`                |
| Rail name, confirm title                         | 16px             | 600     | 19px        | `.pd-name`, `.pd-confirm-box h2`            |
| Row title, body, nav item                        | 13px `--text-md` | 500     | 16px        | `.pk-text strong`, `.pd-field-label strong` |
| Inputs, host buttons, menu item titles, subtitle | 12.5px           | 400/500 | 15px        | `.pd-input`, `.pk-host`, `.pd-title p`      |
| Secondary text, chips, toolbar line, errors      | 12px `--text-sm` | 400     | 15px        | `.pd-chip`, `.pd-alert`, `.np-head p`       |
| Hints, field descriptions, notes                 | 11.5px           | 400     | 14-18px     | `.pd-field-label span`, `.pk-info`          |
| Meta line under a row title                      | 11px `--text-xs` | 400     | 13px        | `.pk-text small`                            |
| Mono meta, small group label                     | 10.5px           | 400/600 | 12-13px     | `.pd-remote`, `.pd-group`                   |
| Eyebrow, column head, list label, tag, count     | 10px             | 500/600 | 12px        | `.pk-eyebrow`, `.pd-row.head`, `.ui-tag`    |

Eyebrows and group labels are uppercase, 600, `letter-spacing: 0.8px`
(`.ui-group-label-text` uses 1.2px). Titles use `letter-spacing: 0`.
Line-heights are fixed pixels, not unitless ratios. `--text-lg/-xl/-2xl`
(18/22/32) exist; 32px is only the full-page `PageFrame` h1, which is
legacy-styled.

## Spacing

Tokens: `--gap-xs 4`, `--gap-sm 7`, `--gap-md 12`, `--gap-lg 18`,
`--gap-xl 26`. Most component CSS writes the concept's pixel values directly;
these are the ones in use:

- inside a row: icon-to-text 7-11px, chip gap 6px, button gap 8px;
- row padding: 8-11px vertical, 10-14px horizontal (`pk-row` 8/10,
  `pd-row` 10/14, `pd-field` 9/0, menu item 7/10);
- between blocks: 12-16px (`pk-where`/`pk-lists` 16px, `pd-toolbar` 20/12,
  `pd-group` 14px above, footer 12-16px above with a 12-14px rule gap);
- dialog padding: compact `22px 24px 16-18px`; two-pane main `22px 32px`
  (Settings: `0 32px 24px` under a 46px sticky strip); rail `18px 10px`,
  209px wide.

## Geometry

| Radius             | Where                                                                                           |
| ------------------ | ----------------------------------------------------------------------------------------------- |
| 4px `--radius-sm`  | checkbox box, kbd, remove buttons, small menu items                                             |
| 6-7px              | inputs (7), buttons (7), chips (7), nav items (7), backend toggle buttons (6), account chip (6) |
| 8px `--radius-md`  | field wells, segmented-control frame, host button, small cards                                  |
| 9px                | `pk-row`                                                                                        |
| 10px `--radius-lg` | tables, cards, banners, popovers, confirm box                                                   |
| 16px               | every dialog frame                                                                              |
| 50%                | dots, step markers                                                                              |

Control heights: `--control-h` is 32px; in practice inputs 31px
(`.pd-input`, select 29px), `.np-input` 33px, `.ui-button` 30px (secondary
32px), chips and host buttons 29px, account chip 24px, branch pill 23px,
`kbd`/tag/count 16px, toggle 32x18.

## Borders, elevation, motion

- Borders are 1px solid; dashed only for "add or drop here" (`.pd-drop`,
  `.pd-server.other`).
- Elevation is the surface step plus `--line`; only floating layers get
  `--shadow-popover`. Dialogs sit on `--scrim` with `backdrop-filter: none`.
- Selected state is drawn inside the box (`border-color` swap or
  `box-shadow: inset 0 0 0 1px var(--line-selected)`), so selecting never
  shifts the rows below.
- Motion: 0.12s on the toggle knob and a disclosure chevron, 0.15s colour and
  background on nav items; the status-dot `pulse` for working sessions. No
  entrance animations.

## States

| State    | Pattern                                                                                                                    |
| -------- | -------------------------------------------------------------------------------------------------------------------------- |
| hover    | row: `background: var(--bg-hover)`; chip/text button: `--text-muted` -> `--text`                                           |
| selected | `--bg-selected` + `--line-selected` border + `--text`, weight 500; nav: `--bg-control`                                     |
| focus    | custom control: `outline: 2px solid var(--focus); outline-offset: 2px`; input: `border-color: var(--focus); outline: none` |
| disabled | `opacity: 0.5` (`0.45` in the new-project footer, `0.4` legacy), `cursor: default`                                         |
| busy     | `.is-busy`: `pointer-events: none; opacity: 0.5`                                                                           |

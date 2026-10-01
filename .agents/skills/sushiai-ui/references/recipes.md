# Recipes for new windows

Every recipe starts the same way: name the nearest existing screen, open its
TSX and its CSS, and build the new one from the same classes. Then run the
concept loop in `SKILL.md`.

## New Settings section

Nearest: `src/app/SettingsDialog.tsx` (General, Providers).

1. If the feature belongs to an extension, contribute it through the
   `settings.section` host (`ExtensionSectionSlot`) instead; nothing in
   `src/app` changes.
2. Otherwise add one entry to `SETTINGS_NAV` (`key`, a sentence-case
   `label`, a lucide `icon`, a one-sentence `description` - the shell renders
   them as the nav item and the page's 20px title and 12.5px subtitle; do not
   render a second title in the section). Add the key to `SettingsTab`.
3. Put the section in its own component, loaded through
   `src/dialogs/lazy-settings.ts` like `ProvidersSettings`.
4. Content uses what Settings already uses: `setting-block` + `h4` for a
   labelled group, `label.setting-check` with an `em` explanation for
   checkboxes, `provider-card`/`claude-accounts` for a group of entities with
   an `add-profile` button in its `provider-card-head`, `p.muted` for one
   line of context, `p.inline-error role="alert"` for failure.
5. A row that is a `form` must out-specify `.modal form` (see below).

`src/app/*` is shell: adding a nav entry is routine, restructuring the dialog
is a question for the owner (`AGENTS.md`, "Vision & boundaries").

## New dialog or window

Nearest: compact - `PanelPickerDialog.tsx` / `WorkspaceDialog.tsx`;
two-pane - `ProjectSettingsDialog.tsx`.

1. Add the kind to the `Dialog` union and `DIALOG_META` (label for
   `aria-label`, a class name such as `foo-dialog`) in
   `src/dialogs/dialog-state.ts`.
2. Render it inside `DialogHost` in `src/App.tsx`. App.tsx is capped at 600
   lines (`tests/app-boundary.test.cjs`) and is near it: render one component
   per branch and keep logic in that component's file.
3. New `src/styles/components/foo.css`, imported in `src/main.tsx`:

   ```css
   /* The Foo dialog. Sizes follow the compact dialogs; colours are tokens. */
   .modal-backdrop:has(.foo) {
     background: var(--scrim);
     backdrop-filter: none;
   }
   .modal.foo-dialog {
     width: 640px;
     max-width: calc(100vw - 48px);
     padding: 22px 24px 18px;
     border-color: var(--line);
     border-radius: 16px;
     background: var(--bg-dialog);
   }
   .foo-dialog > .modal-close {
     top: 23px;
     right: 24px;
     width: 15px;
     height: 15px;
     color: var(--text-muted);
   }
   .foo .foo-head h2 {
     margin: 0;
     font-size: 18px;
     font-weight: 600;
     letter-spacing: 0;
     line-height: 22px;
   }
   ```

4. Structure: head (eyebrow, title, optional subtitle) -> body built from
   catalogue rows -> footer with a muted sentence and the actions. One
   primary action. Escape on an inner layer (menu, inline edit) calls
   `event.stopPropagation()` so the dialog stays open.
5. Two-pane: copy the shape, not the classes - if the window is a second
   "settings-like" window, that is the moment to promote the rail/page pair
   to a shared primitive.

## New project-settings tab

Nearest: `src/ProjectGeneralTab.tsx` (rows) or `src/ProjectEnvironmentTab.tsx`
(table).

1. Add the tab to `Tab`/`TABS` in `src/ProjectSettingsDialog.tsx` with a
   lucide icon (15px in the rail), and to `ProjectSettingsTab` in
   `src/app/openSettings.ts` if something must deep-link to it.
2. Wrap the tab in `ProjectPage` with a title and a one-sentence subtitle
   that says what the page controls for every host.
3. Rows: `h3.pd-group` + `Field`; records: `pd-toolbar` (count sentence on the
   left, `ui-button secondary` actions on the right) + `pd-table`; notices:
   `Banner`; failure: `pd-alert`; nothing yet: `pd-empty` or `pd-empty-row`.
4. Styles go in `project.css` under a comment naming the tab, scoped under
   `.pd`.

## New menu or popover

Nearest: the account menu in `PanelPickerDialog.tsx`.

1. Trigger: a chip button with `aria-haspopup="menu"`, `aria-expanded`, and a
   `ChevronDown size={11}`; style from `.picker-account-chip`.
2. Wrap trigger and menu in a `position: relative` span; the menu uses the
   `.picker-account-menu` box values and `pk-menu-group`, `pk-menu-item`
   (`role="menuitemradio"` + `aria-checked`, trailing `Check size={12}`),
   `pk-menu-rule`, `pk-menu-foot`.
3. Close on pick, on the trigger, and on Escape with `stopPropagation`. If it
   must also close on blur, first check that
   `event.relatedTarget` is outside `event.currentTarget`, as
   `ProjectEnvironmentTab.tsx` does for drag state.
4. A dialog that clips it needs `overflow: visible` on the `.modal` (the
   picker does this, with `overflow: auto` back under 640px of height).
5. If the menu lives outside the picker, promote the `pk-menu-*` rules to a
   shared `ui-menu` primitive in the same change rather than writing a third
   copy next to `np-menu`.

## New list row

Nearest: `pk-row` (a choice), `pd-row` (a record with columns),
`ui-task-row` (an orchestrator task), `.workspace-item` (sidebar).

- One button covers the row; extra controls are siblings, layered above it.
- Title 13px/500 `--text`, meta 11px `--text-faint`, both single-line with
  ellipsis and `min-width: 0` on the flex child.
- Provenance or state: one `Tag` or `ui-dot` at the name, details in `title`.
- Selected/focused: draw it inside the box (border swap or inset shadow) so
  rows never shift.
- Hover-only actions use `:hover` and `:focus-within` together, so keyboard
  users see them.

## Specificity gotchas

- **`.modal form` stacks any form as a column** (`display: flex;
flex-direction: column; gap: 17px; margin-top: 24px`, specificity 0,1,1).
  A bare `.my-row { display: grid }` (0,1,0) loses. The Providers account
  rows were fixed with `.providers-settings .claude-account-row` (0,2,0)
  plus `margin-top: 0`. Any `form` used as a row inside a dialog needs the
  same two-class selector. (`form.pd-row.pd-adding` in
  `ProjectEnvironmentTab.tsx` is styled only by single-class rules - check it
  in a screenshot before reusing it.)
- **`.modal label`** is a flex column and **`.modal input`** is
  `width: 100%`; a checkbox label must out-specify both (`.modal
label.setting-check`, `.modal input.ui-toggle[type="checkbox"]`).
- **`.modal h2`** is 25px/500 with `letter-spacing: -0.9px`; every new title
  restates size, weight, `letter-spacing: 0` and margin under a two-class
  selector (`.pk .pk-head h2`).
- `.primary:not(.ui-button)` and `.secondary:not(.ui-button)` are the legacy
  buttons; `ui-button` opts out of them, so never combine `primary` without
  `ui-button` on a new screen.
- Prefer a root class plus element class (`.pd .pd-input`) over `!important`
  or ids.

## Slop tells, with the counter-example from the app

| Tell                                        | Do instead                                           |
| ------------------------------------------- | ---------------------------------------------------- |
| decorative gradient, glow, blur             | flat surface step; blur is off even on the scrim     |
| emoji or a coloured icon as decoration      | lucide at the role size in `--text-muted`            |
| a 24-32px heading in a dialog or pane       | 18px (compact) or 20px (two-pane) at 600             |
| card inside a card                          | one bordered box; group with `pd-group` labels       |
| centred stack of full-width labelled inputs | label-left `Field` rows                              |
| the same status pill on every row           | one `Tag` where it differs; tone dot otherwise       |
| a category heading row per host or kind     | an icon or tag at the name, full text in `title`     |
| a new hex, shade, radius or shadow          | a token; a radius from the table in `tokens.md`      |
| 12, 14 and 16px icons in one row            | one size per role                                    |
| buttons of 28, 30, 36px side by side        | `ui-button` 30/32 as shipped                         |
| "Here you can manage your..." subtitles     | what the page controls, in one sentence              |
| Title Case Labels, exclamation marks        | sentence case, plain statements                      |
| a spinner where a sentence would do         | `.loading` text or a progress line                   |
| a third copy of a menu/segmented/input      | reuse, or promote to `ui-*` and move the callers     |
| a plugin page that hides the sidebar        | contributed pages render where `SectionPage` renders |

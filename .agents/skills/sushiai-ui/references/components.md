# Component catalogue

Each entry: the classes, the canonical file to open before reuse, when to use
it, and a snippet copied from the shipped code (trimmed, never invented).

## Dialog shell

`src/dialogs/DialogHost.tsx` draws every dialog: `.modal-backdrop` (closes on
a mousedown on the backdrop itself), `.modal <className>` with
`role="dialog"`, `aria-modal`, `aria-label`, and the `.modal-close` X
(`<X size={17} />`). A dialog registers its kind, label and class in
`DIALOG_META` (`src/dialogs/dialog-state.ts`). The dialog never closes itself;
the caller closes it after the action succeeds.

Per-dialog framing lives in that dialog's area file
(`src/styles/components/picker.css`):

```css
.modal-backdrop:has(.pk) {
  background: var(--scrim);
  backdrop-filter: none;
}
.modal.command-modal {
  width: 640px;
  max-width: calc(100vw - 48px);
  padding: 22px 24px 16px;
  border-color: var(--line);
  border-radius: 16px;
  background: var(--bg-dialog);
}
.command-modal > .modal-close {
  top: 33px;
  right: 24px;
  width: 15px;
  height: 15px;
  color: var(--text-muted);
}
```

Two sizes exist: **640px compact** (new session picker `command-modal`, new
project `np-dialog`) and **940px two-pane** (Settings, project settings
`project-dialog`, 720px tall, `max-height: calc(100dvh - 48px)`).

## Two-pane window: rail + page

Canonical: `src/ProjectSettingsDialog.tsx` + `src/ProjectPage.tsx` (CSS
`project.css`); Settings is the same shape (`src/app/SettingsDialog.tsx`,
`src/app/settings-dialog.css`). Rail 209px on `--bg-sunken` with a head
(eyebrow, 16px name, mono meta line) and a nav of 30px buttons with a 15px
icon; main column with a fixed title and a scrolling body.

```tsx
<div className="pd">
  <aside className="pd-rail">
    <div className="pd-head">
      <span className="pd-eyebrow">PROJECT</span>
      <strong className="pd-name" title={workspaceName}>
        {workspaceName}
      </strong>
      <span className="pd-remote">{repoSlug(project.git.url)}</span>
    </div>
    <nav aria-label="Project settings" className="pd-nav">
      {TABS.map(([item, Icon]) => (
        <button
          key={item}
          className={tab === item ? "selected" : ""}
          aria-current={tab === item ? "page" : undefined}
          onClick={() => setTab(item)}
        >
          <Icon size={15} aria-hidden />
          {item}
        </button>
      ))}
    </nav>
  </aside>
  <main className="pd-main">{/* a ProjectPage per tab */}</main>
</div>
```

```tsx
export function ProjectPage({ title, subtitle, children }) {
  return (
    <>
      <header className="pd-title">
        <h2>{title}</h2>
        <p>{subtitle}</p>
      </header>
      <div className="pd-body">{children}</div>
    </>
  );
}
```

`.pd-body` scrolls inside the page padding (`margin-right: -32px`), fades its
head and foot with a mask, and uses a thin `--line` scrollbar. Settings uses
`role="tablist"`/`role="tab"`/`aria-selected` on its nav and a 46px opaque
sticky strip so content never slides under the close X.

## Compact dialog head and footer

Canonical: `PanelPickerDialog.tsx` (`pk-head`, `pk-foot`), `WorkspaceDialog.tsx`
(`np-head`, `np-footer`). Head: a 10px uppercase eyebrow, an 18px/22px 600
title, an optional 12px muted subtitle; `padding-right: 25px` keeps the title
clear of the X. Footer: `margin-top: 12-16px; padding-top: 12-14px;
border-top: 1px solid var(--line-divider)`, a flexible muted sentence on the
left saying what the primary action will do, then the action or key hints.

```tsx
<footer className="pk-foot">
  <span>{`${agentTitle(agent)} on ${launchLabel}`}</span>
  <kbd className="pk-kbd">⏎</kbd>
  <span className="pk-foot-text">start</span>
  <kbd className="pk-kbd">esc</kbd>
</footer>
```

## Labelled setting row (`Field`)

Canonical: `src/ProjectGeneralTab.tsx`, CSS `project.css` (`pd-field`). Label
column 230px (13px/500 label, 11.5px muted hint), 24px gap, control fills the
rest; `padding: 9px 0`, `--line-divider` under each row. Group rows under an
uppercase `h3.pd-group`. Use it for any "one setting = one row" page.

```tsx
<h3 className="pd-group">Repository</h3>
<Field
  label="Default branch"
  hint="Tasks branch from here and land back into it."
>
  <input
    className="pd-input"
    aria-label="Default branch"
    value={project.git.defaultBranch}
    placeholder="main"
    onChange={...}
    onBlur={commit}
  />
</Field>
```

`Field` is local to `ProjectGeneralTab.tsx` today; a second file that needs
it is the moment to export it, not to copy it. On the General page values
save on blur or change; there is no Save button. An inline failure goes
under the control as `<span role="alert" className="pd-alert">`.

## Inputs and selects

- `.pd .pd-input` (`project.css`): 31px, `padding: 7px 10px`, radius 7px,
  `--bg-app`, `--line` border, 12.5px; focus swaps the border to `--focus`.
  `select.pd-input` is 29px with a drawn chevron. `.pd-mono` for keys and
  commands.
- `.np .np-input` (`new-project.css`): the same control at 33px, radius 8px,
  for the new-project flow; `.np-field` wraps an input with a leading icon.
- Legacy `.modal input/select` (`styles.css`): 100% wide, `#202020`, 10px
  padding - what an unstyled input in a dialog gets. Give new inputs a class.
- Chip list (`.pd-chips` + `.pd-chip`, `ProjectGeneralTab.tsx` Network): a
  `--bg-app` well of 29px chips; hover shows a `.pd-chip-x`; the last chip is
  `+ Add` and turns into an inline input.

## Choice list row (`pk-row`)

Canonical: `PanelPickerDialog.tsx`, CSS `picker.css`. A row is a `div` holding
one full-row `button.pk-row-main` (its `::after` covers the row, so the whole
row clicks) plus optional siblings above it (an account chip, a `kbd`). 17px
glyph, two-line text, `padding: 8px 10px`, radius 9px; hover `--bg-hover`;
the keyboard-focused row gets `--bg-selected` and a `--line-selected` border.
Lists sit under a `.pk-label` (10px uppercase).

```tsx
<div className="pk-row" key={item.kind}>
  <button className="pk-row-main" disabled={adding} onClick={...}>
    <item.icon size={17} aria-hidden className="pk-icon" />
    <span className="pk-text">
      <strong>{item.title}</strong>
      <small>{item.detail}</small>
    </span>
  </button>
  <kbd className="pk-kbd">{item.key.toUpperCase()}</kbd>
</div>
```

A warning in the meta line is `small.is-warning` (`--tone-warning`), not a
badge.

## Selectable buttons, segmented control, checkbox

- Host/option buttons: `.pk-host` (29px, radius 8, `--bg-control`) in a
  `role="radiogroup"`, each `role="radio"` + `aria-checked`, with a
  `ui-dot ui-tone-*` before the label.
- Segmented: `.pk-backend` (frame `--bg-app`, 2px padding; buttons 4/10px,
  selected `--bg-selected`, `aria-pressed`). Near-copies exist as `.np-tabs`
  and legacy `.workspace-control-tabs` (Settings "Interface size") - do not
  add a fourth.
- Checkbox: `label.pk-worktree` with a hidden input and a drawn 15px
  `.pk-box` (radius 4, accent fill when on, `Check size={11} strokeWidth={3}`).
- Switch: `Toggle` (`src/orchestrator/ui/Toggle.tsx`) - a real checkbox drawn
  32x18; it needs a `label` prop for its accessible name.

## Popover menu

Canonical: the account menu in `PanelPickerDialog.tsx`
(`.picker-account-chip` trigger, `.picker-account-menu` list, `.pk-menu-*`).
Trigger is a 24px chip with `aria-haspopup="menu"`, `aria-expanded` and a
`ChevronDown size={11}`; open/focus border `--focus`. The menu is absolute at
`top: calc(100% + 6px)`, `min-width: 320px`, `padding: 5px`, `--line-strong`
border, radius 10, `--bg-elevated`, `--shadow-popover`.

```tsx
<div
  className="picker-account-menu"
  role="menu"
  aria-label="Claude Code runs as"
  onKeyDown={(event) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      setAccountMenu(false);
    }
  }}
>
  <div className="pk-menu-group">Claude Code runs as</div>
  <button
    role="menuitemradio"
    aria-checked={selected}
    className="pk-menu-item"
    onClick={() => pick({ account: item.id })}
  >
    <span className="pk-text">
      <strong>{item.label}</strong>
      <small>Subscription · logged in</small>
    </span>
    {selected && <Check size={12} aria-hidden />}
  </button>
  <div className="pk-menu-rule" />
  <button role="menuitem" className="pk-menu-foot" onClick={...}>
    Add a subscription or a key in Settings → Providers
  </button>
</div>
```

The selected item gets `--bg-selected` and a trailing `Check`; the footer
link is `--tone-info`. `.np-menu`/`.np-select` in `new-project.css` is an
earlier copy of the same thing, and `.pk-project-menu` a smaller variant.

## Table of records

Canonical: `ProjectEnvironmentTab.tsx` / `ProjectHostsTab.tsx`, CSS
`project.css`. `.pd-table` is the bordered box (radius 10); `.pd-row` is one
CSS grid line (`padding: 10px 14px`, 11.5px, `--line-divider` between rows);
`.pd-row.head` is the 10px uppercase header on `--bg-raised`; each table sets
its columns with one rule (`.pd-env .pd-row { grid-template-columns: ... }`).
Use `role="table"`, `role="row"`, `role="columnheader"`. A row's remove
button appears on hover or focus-within (`.pd-row-remove`). Empty: a
`p.pd-empty-row` inside the table.

## Buttons

| Class                 | Look                                                 | Where                                        |
| --------------------- | ---------------------------------------------------- | -------------------------------------------- |
| `ui-button primary`   | accent fill, accent ink, 30px, radius 7, 13px/500    | the one main action of a new screen          |
| `ui-button secondary` | `--bg-raised`, `--line-strong` border, 32px          | Cancel, toolbar actions (`Import .mcp.json`) |
| `ui-button ghost`     | transparent, hover `--bg-hover`                      | low-weight inline action                     |
| `.primary` (legacy)   | light grey `#e0e0dc`, dark text, 12px/550            | Settings, Providers, `Empty` action          |
| `.secondary` (legacy) | `#272727`, 12px                                      | Settings forms                               |
| `.add-profile`        | text button, 11px, `#aaa` -> white, `Plus size={12}` | "Add account" in a section head              |
| `.danger` (legacy)    | red fill                                             | rare destructive confirm                     |
| `.icon-button`        | 25px square, radius 5                                | icon-only actions; needs `aria-label`        |

`ui-button` comes from `src/orchestrator/ui/ui.css`, which loads when a module
imports from `src/orchestrator/ui` - import a primitive from there in the
file that uses the classes. New screens use `ui-button`; inside an existing
legacy screen (Settings, Providers) match the buttons already on it rather
than mixing both families on one screen. Actions sit right-aligned, Cancel
before the primary (`.pd-confirm-actions`, `.dialog-actions`).

## Status: dots, tags, counts, banners

```tsx
<span className={`ui-dot ui-tone-${hostTone(host.workspace)}`} />
<Tag tone="ok">ready</Tag>
<Tag tone="neutral" dot={false}>...</Tag>
<Banner tone="info" title="..." body="..." />
```

- `ui-dot`: 6px circle in the tone colour, before a label (host buttons).
- `Tag` (`ui-tag`): 16px pill, 10px/500, tone text on tone-bg, leading 5px
  dot unless `dot={false}`. One per item, at the name.
- `ui-count`: 16px count badge next to a group label (`GroupLabel`).
- `Banner` (`ui-banner`): toned box with an 8px mark, a 13px/500 title, a
  12px body and at most one `ui-button secondary` action; `role="alert"`
  unless `tone="info"`. For a notice the user should read, not for errors in
  a form.
- Legacy `.status-dot` (4px; `green`, `yellow`, `red`, `blue`, `pulse`) is
  the sidebar's live/working marker. Keep it there; new UI uses `ui-dot`.

## Key hints

`kbd.pk-kbd` (`picker.css`): 16px tall, `min-width: 19px`, radius 4,
`--bg-count`, `--line` border, 10.5px muted. Content is the key itself:
`1`, `T`, `⏎`, `esc`. Legacy `.dialog-footer kbd` and `.kbd` (mascot) are
older variants.

## Errors and alerts

- `.pd-alert`: plain `--danger` text, 12px/15px, `role="alert"`, under the
  control or at the top of the page (`ProjectSettingsDialog.tsx`).
- `.np-error`, `.pk-error`: the same idea in those dialogs.
- `.inline-error` (legacy, Settings and Providers): a bordered dark-red well.
  Keep using it inside Settings; do not introduce it elsewhere.
- In-dialog confirm (`.pd-confirm` overlay + `.pd-confirm-box`, 420px): an
  eyebrow (`dialog-eyebrow`), a 16px question naming the object ("Close
  {name}?"), one muted paragraph stating exactly what is lost, then Cancel
  (`secondary`) and the action (`primary`). No nested modal.

## Empty and loading

- Page or canvas: `Empty` (`src/app/Empty.tsx`): lucide icon at 30px, an 18px
  title, a 12px line (max 370px), an optional `primary` action with
  `Plus size={14}`. Example: `title="Space for your next idea."
text="Add a panel to get started." action="Add panel"`.
- Inside a page: `p.pd-empty` (12px muted sentence that says what to do);
  inside a table: `p.pd-empty-row` (centred, 28px padding).
- Loading: `.loading` text ("Loading settings…") as the `Suspense` fallback.

## Icons

lucide-react only, imported by name. Sizes in use, by role:

| Size  | Role                                                                |
| ----- | ------------------------------------------------------------------- |
| 10-11 | provenance markers at a sidebar name, chevrons on chips             |
| 12    | inline with 11-12px text, `Check` in menus, `Plus` in `add-profile` |
| 13    | sidebar row actions (`MoreHorizontal`, `Settings`), menu checks     |
| 14    | icon in a labelled button (`Plus`, `RefreshCw`), title chevrons     |
| 15    | nav rail items, dialog close in compact dialogs                     |
| 16-17 | settings note icon, picker row glyphs, the dialog close X           |
| 30    | empty-state illustration                                            |

Agent marks are SVG images through `Icon` in `src/PanelIcon.tsx`
(`<Icon kind="agent" agent="claude" size={17} />`); extension icons through
`ExtensionIcon`. Icons take the text colour of their context (usually
`--text-muted`), never a colour of their own.

## Sidebar

`src/app/Sidebar.tsx`. A workspace row is the name with a chevron
(`size={12}`), then provenance as a 10px icon or a `.remote-tag` right after
the name, then a 4px `status-dot` for a live session. Hosts and kinds are
marked at the name with the full text in `title`; there are no per-category
heading rows and no repeated status pills. Extensions add actions to the
sidebar through declared slots (`ExtensionActionSlot`); they never replace
the sidebar or the primary nav.

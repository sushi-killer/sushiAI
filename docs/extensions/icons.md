# Extension icons

An icon is either a bundled name or an svg object
`{"kind":"svg","viewBox":"0 0 24 24","paths":["M…"]}`. Only path data is kept.

## Bundled names

`list-check`, `list-todo`, `layout-grid`, `plug`, `sparkles`, `workflow`,
`terminal`, `folder`, `key-round`, `file-text`, `clock`, `database`,
`shield-check`, `bot`, `globe`, `server`.

An unknown name is rejected with "is not a bundled icon".

## Where to declare one

- `icon` at the top level of the manifest is the extension's default icon.
- `icon` on a surface, a navigation entry or an action sets that item's own icon.

Inherit rule: a surface or navigation entry without its own `icon` takes the
extension's top-level `icon`; its own icon always wins. Actions do not inherit.
Without either, a surface falls back to `plug`. The field is optional and
additive, so the Extension API version stays 1.

## Rendering

Named and svg icons draw with one stroke width (1.8) in a fixed square box.
The default size is 14; Settings tabs use 15 like the core tabs.

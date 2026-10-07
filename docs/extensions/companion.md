# Companion processes

A companion is a native program that serves the values of one extension view.
The extension folder carries no code. The program is installed outside it.
The example here is the fixture `tests/fixtures/extensions/companion-probe`.

## Manifest

```json
"companion": {
  "command": "probe-companion",
  "args": ["serve"],
  "permissions": ["hosts.read"]
}
```

- `command` is a bare name. The app looks in `$SUSHIAI_HOME/bin`, then in the app PATH.
- The app refuses a command whose real path is inside the extension folder.
- `args`: at most 8 strings, 200 characters each.
- `permissions`: only `hosts.read` exists.
- A built-in extension never declares `companion`.

The view is a surface on the host `settings.page`:

```json
"view": {
  "kind": "companion",
  "fields": [
    { "id": "service", "label": "Service", "type": "status" },
    { "id": "pairing", "label": "Pairing code", "type": "qr" },
    { "id": "note", "label": "Note", "type": "text" }
  ],
  "actions": [
    { "id": "add-device", "label": "Add device", "method": "device.add" },
    { "id": "import-hosts", "label": "Import hosts", "method": "hosts.import", "send": ["hosts"] }
  ]
}
```

The Settings dialog shows the surface as one tab.

## Approval

The program starts only when all three are true:

1. The extension is enabled.
2. The owner approved the resolved path, the args and the permissions.
3. The app is ready.

Until then the state is "needs approval". The Extensions page shows the
resolved path, the args and the permissions. A change of any of them asks
again. There is no binary hash, so a program update does not ask again.
Approval is consent, not a sandbox: the program keeps the owner's OS rights.

## Protocol

The program talks over stdin and stdout with the same frames as the sushiai daemon.

1. The app sends `hello {protocol: 1, client: "sushiai-desktop"}`.
   The program replies `{protocol: 1}`.
2. The app sends `view.read {surfaceId}`. The program replies `{values}`, one
   value per field id.
3. The app sends `<action.method> {surfaceId, hosts?}` when the owner presses a
   button. The program replies `{values?, message?}`.
4. The program may send the notification `view.changed {surfaceId}`. The app
   then calls `view.read` again.

The program sends no other request to the app.

Values:

- `text`: a string, at most 1000 characters.
- `status`: `{text, tone}`. `tone` is one of the known tones.
- `qr`: a string, at most 2048 characters. The app draws the QR code. It never
  accepts image bytes.
- `null` means empty.

The app never stores or logs values.

## Hosts

An action with `send: ["hosts"]` needs `hosts.read` in `permissions`. The app
then adds `params.hosts`: a list of `{id, name, host, port?}` for the saved ssh
hosts. It leaves out hosts that use a command connector. A profile holds no secret.

## Limits and lifecycle

- Timeouts: 10 s for `hello` and `view.read`, 30 s for an action.
- The environment has only `PATH`, `HOME`, `USER`, `LANG`, `TMPDIR` and `SUSHIAI_HOME`.
- After an exit, the app restarts the program after 1 s, then 5 s, then 30 s.
- After 3 exits in 10 minutes, the state is "failed". The Extensions page shows
  the end of stderr. A new approval or a disable clears it.
- On disable or quit, the app sends SIGTERM, waits 3 s, then sends SIGKILL.
- The manifest limits: at most 8 fields, 4 actions, unique action labels.

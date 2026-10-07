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
- `permissions`: `hosts.read` and `hosts.exec`. `hosts.exec` needs `hosts.read`.
- A built-in extension never declares `companion`.

The view is a surface on the host `settings.page`:

```json
"view": {
  "kind": "companion",
  "fields": [
    { "id": "service", "label": "Service", "type": "status" },
    { "id": "pairing", "label": "Scan code", "type": "qr" },
    { "id": "note", "label": "Note", "type": "text" },
    { "id": "hosts", "label": "Hosts", "type": "list", "method": "host.add" }
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
5. With `hosts.read`, the app sends the notification `hosts.changed {hosts}`
   when the program starts and whenever the saved ssh host list changes. The
   program may ignore it.
6. With `hosts.exec`, the program may send the request `host.exec` (see
   "Running commands on a host").

The program sends no other request to the app.

Values:

- `text`: a string, at most 1000 characters.
- `status`: `{text, tone}`. `tone` is one of the known tones.
- `list`: see "List field".
- `qr`: a string, at most 2048 characters. The app draws the QR code. It never
  accepts image bytes.
- `null` means empty.

The app never stores or logs values.

## List field

`{ "id": "hosts", "label": "Hosts", "type": "list", "method": "host.add" }`.
A view has at most one list field. It counts toward the 8 fields. `method` is
optional and matches the same pattern as an action method.

The value is a list of rows:

```json
[
  {
    "id": "h1",
    "label": "ra2",
    "detail": "user@devbox:22",
    "tone": "ok",
    "status": "In Remote",
    "action": "Add to Remote"
  }
]
```

- At most 50 rows. `id` is at most 100 characters. `label`, `detail`,
  `status` and `action` are at most 200 characters. No control characters.
  A row that breaks a rule is dropped. The rest of the value stays.
- `tone` is one of the known tones. An unknown tone keeps the row and shows as
  `neutral`.
- A row with `action` shows a button when the field has a `method`. The button
  calls that method with `{surfaceId, row: "<row id>"}`. The 30 s action
  timeout applies, so a long job answers at once and reports progress by
  changing the rows and sending `view.changed`.
- When the program sends `view.changed`, the app reads the whole view again,
  lists included. An error from a row button shows like an action error.
- The list is Settings content. It never draws chrome.

## Hosts

An action with `send: ["hosts"]` needs `hosts.read` in `permissions`. The app
then adds `params.hosts`: a list of `{id, name, host, port?}` for the saved ssh
hosts. It leaves out hosts that use a command connector. A profile holds no secret.

## Running commands on a host

A program with `hosts.exec` can run a command on a saved ssh host through the
app. The app uses its own ssh connection (ssh-agent and `~/.ssh/config`).
Request:

```json
{
  "host": "<host id>",
  "title": "<up to 120 characters>",
  "argv": ["sh", "-c", "uname -m"],
  "stdin": "<base64, optional>",
  "timeoutMs": 120000
}
```

Reply: `{ "code": <int or null>, "stdout": "...", "stderr": "..." }`. The app
keeps the last 64 KiB of each stream. `code` is null after a timeout.

- `local` and unknown hosts are refused. So are hosts that use a command
  connector.
- `stdin` is at most 11 MiB decoded: the whole JSON request must fit one 16 MiB
  frame. `argv` is at most 64 Ki characters in total. `title` has no control or
  invisible characters. A request over these limits fails with `-32602`. A frame
  over 16 MiB is a protocol error and ends the connection; the app cannot fail
  that one request alone, so a program must check the size first.
  `timeoutMs` is at most 300000.
- The app sends the command to the host as fixed text plus base64 that the
  host's `/bin/sh` decodes and evals, so every login shell (sh, bash, zsh,
  fish) passes the same words that the card showed.
- **Every call needs its own owner confirmation.** There is no standing
  allowance. The app draws a card outside the extension's tab. It shows the
  extension name and id, the host name and address, the exact argv (monospace,
  scrollable, never cut), the `title` labelled "Extension's description" (the
  extension's own words, not verified), and for stdin only its size and
  SHA-256.
- Deny is the default. Enter and Escape deny. Allow needs a mouse click (or Tab
  to the button and Space), and the button is disabled for the first second.
  No answer in 2 minutes refuses; the 2 minutes start when the card shows.
- The app owns the queue: one card is on screen at a time across all programs.
  A program has one card on screen and at most one call waiting behind it;
  more calls fail with `-32002`. The app ignores an Allow earlier than 1 s
  after the card was shown. The card closes when it times out or the program
  restarts, and a late answer does nothing. If the saved host changed while the
  card was open, the call is refused.
- A call counts its `timeoutMs` from when it starts to run.
- The card escapes every control, invisible and line-separator character in the
  text the program supplied.
- `hosts.changed` never lists `local`.
- At most one command per host and 4 in total.
- The app never logs or stores argv, stdin or output text. The audit line holds
  the extension id, host id, SHA-256 of argv, SHA-256 of stdin, the decision
  (`allowed`, `denied`, `timeout`, or `busy` when an allowed call found the host
  taken), the exit code and the time.

**What the card is, and is not.** The card is consent. It protects against an
honest or buggy program. It is not a sandbox: an approved program already runs
with your OS rights and could read `~/.ssh` and connect on its own. The real
protection against a malicious program is not approving it.

Error codes: `-32001` the owner refused, `-32002` busy, `-32003` no
`hosts.exec` permission, `-32004` unknown or unsupported host, `-32602` bad
parameters, `-32000` the run failed.

## Limits and lifecycle

- Timeouts: 10 s for `hello` and `view.read`, 30 s for an action.
- The environment has only `PATH`, `HOME`, `USER`, `LANG`, `TMPDIR` and `SUSHIAI_HOME`.
- After an exit, the app restarts the program after 1 s, then 5 s, then 30 s.
- After 3 exits in 10 minutes, the state is "failed". The Extensions page shows
  the end of stderr. A new approval or a disable clears it.
- On disable or quit, the app sends SIGTERM, waits 3 s, then sends SIGKILL.
- The manifest limits: at most 8 fields, 4 actions, unique action labels.

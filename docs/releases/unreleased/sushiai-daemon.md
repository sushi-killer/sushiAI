## Sessions live in the sushiai daemon

- Terminals and agents now run in sessions of the `sushiai` daemon, a small background process the app starts for you. Sessions survive an app restart or crash: open sushiAI again and every panel reattaches to its session with the same screen and layout.
- Remote hosts get `sushiai` from **Settings → Connections**: choose **Install sushiai** on the host card and sushiAI uploads the matching build over SSH, verifies its checksum and starts it. A host reached through a local command can be added too. An unreachable host, a changed host key or a login that needs attention is reported on its card without retrying in a loop.
- The Inbox shows an agent's permission requests with **Allow** and **Deny** buttons; answering one sends the decision to the waiting agent. An agent opens a Preview next to its pane with `sushiai open`.
- Herdr is no longer needed and is no longer used: there is no Herdr setting, extension, CLI check or install step, and the "Herdr 1.0.0" entry is gone from Extensions.
- Breaking: panels saved by an earlier release that ran in Herdr cannot be reattached. They come back as **Session ended** with **Reopen**, which starts a new session (and resumes an agent's own session when it has one). Sessions that were still running in Herdr are not adopted; end them with Herdr itself.
- Codex sessions need codex-cli 0.160 or newer (the release with `--no-daemon`); host setup installs the current Codex on a remote host that has none.

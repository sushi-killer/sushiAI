## Agents

- Idle Claude and Codex sessions now sleep after the time set in Settings → General → Idle agents (off, 1 h, 4 h or 12 h; default 4 h). A sleeping pane keeps its last screen. Type in it or click it and the agent resumes where it left off, with its saved launch settings.
- Panel menu → Keep awake stops one agent from ever sleeping. A focused agent never sleeps either.
- After a reboot or a crash, agents wake on demand instead of ending: the pane shows its last screen and resumes when you type or press Reopen.
- Launch settings (variables, model, extra arguments) needed to wake an agent are stored encrypted on your machine. The key lives in the macOS login Keychain, or in a private key file on Linux.
- A background process the agent started (for example a dev server) stops when the agent sleeps. Use Panel menu → Keep awake for an agent that must keep one running.
- The idle-sleep setting in the app is the master for every host it connects to: the app sends it again each time it connects, so a value set on a host by other means is replaced.
- The state file moves to schema 2. An older app's daemon refuses it, so after a downgrade move `~/.sushiai/state.json` aside.

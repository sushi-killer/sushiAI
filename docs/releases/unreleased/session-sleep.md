## Agents

- Idle Claude and Codex sessions now sleep after the time set in Settings → General → Agents (off, 1 h, 4 h or 12 h; default 4 h). A sleeping pane keeps its last screen. Type in it or click it and the agent resumes where it left off, with its saved launch settings.
- Panel menu → Keep awake stops one agent from ever sleeping. A focused agent never sleeps either.
- After a reboot or a crash, agents wake on demand instead of ending: the pane shows its last screen and resumes when you type or press Reopen.
- Launch settings (variables, model, extra arguments) needed to wake an agent are stored encrypted on your machine. The key lives in the macOS login Keychain, or in a private key file on Linux.

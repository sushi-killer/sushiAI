## Accounts

- Codex can have several accounts. Add them in Settings → Providers → Codex accounts: Sign in opens ChatGPT in your browser, or paste an OpenAI API key. Each account keeps its own Codex login and shares your Codex settings, skills and history.
- A project can pick its default Codex account in Project settings → General, next to its Claude Code account. The + picker starts Codex as that account, and a chip on the Codex row changes it for one session.
- On an SSH host, a session started as a Codex account uses a copy of that account's login for that session only, even where the host has a login of its own. A ChatGPT login that refreshes during such a session may ask you to sign in to that account again.
- The project's Codex account applies to terminal and Herdr sessions; orchestrator tasks and Codex threads still run on the default Codex login.
- The Claude and Codex account rows in Settings → Providers now line up on one line instead of stacking.

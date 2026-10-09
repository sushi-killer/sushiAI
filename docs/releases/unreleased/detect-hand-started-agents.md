## Terminals

- A terminal now shows an agent you start by hand in it. Type `claude`, `codex`, `gemini` or `cursor-agent` in a shell pane and the pane gets that agent's icon and attention state within a few seconds; the icon goes away when the agent quits back to the shell.
- Reopen continues such an agent when its conversation is known, in the folder it ran in and with your own login. Claude Code continues the exact conversation. Codex continues only when you started it with `--no-daemon`; a Codex started by hand in its default mode cannot be told apart from the others, so its pane reopens as a plain shell. A pane whose conversation cannot be told never guesses: it reopens as a plain shell. Gemini and cursor-agent start again.

## Skills have one owner

- The `sushiai` binary now owns every skill folder: `sushiai skills --json --catalog` is the Skills screen's catalog, and `sushiai skills install|remove NAME` installs and removes the skills sushiAI ships (`sushiai-artifacts`, `sushiai-routines`, `sushiai-orchestrator`). The app runs the binary instead of writing folders itself.
- A builtin skill is installed once per harness: into Claude Code's folder and into Codex's `$CODEX_HOME/skills` only. An older copy of ours in `~/.agents/skills` is removed, so Codex no longer loads it twice.
- Nothing is installed for a tool that is not there: without `~/.codex`, no `~/.codex` or `~/.agents` folder is created. A skill folder whose SKILL.md declares another name, or a linked skill folder, is never overwritten or removed; `sushiai orch unregister` no longer deletes a skill folder it did not write.
- The Skills screen lists only Claude Code and Codex. Skills of Cursor, Gemini, Windsurf and the other harnesses, the "Other agents" group and the "External harness" filter are gone.
- A skill linked into several folders of one harness is one row. A row is a duplicate only when one harness sees the same name twice: a Claude Code copy and a Codex copy are not, and a disabled plugin skill never is.
- On an SSH host the skills are installed by the host's own `sushiai` after it is installed, so a fresh host gets them in the same setup.

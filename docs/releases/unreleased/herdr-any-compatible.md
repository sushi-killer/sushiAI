## Herdr

- sushiAI now installs Herdr 0.9.3, and works with any Herdr version that still offers what it uses. A newer or older Herdr (0.8.2 included) no longer reads as incompatible just because its version or protocol number differs; only a missing capability does.
- When sushiAI's own Herdr CLI speaks another protocol than the Herdr server you run, the terminal attaches through your own Herdr CLI, or a release sushiAI installed earlier, instead of failing. This works on SSH hosts too.
- On start, a Herdr workspace saved with no live session and nothing else open in it moves to Recently closed instead of staying as a second row of its project. This clears rows such as worktrees whose folder was deleted and empty workspaces of an unreachable SSH host. While the app runs, a workspace whose sessions ended still keeps its Reopen in place.

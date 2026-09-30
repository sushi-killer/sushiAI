## A worktree per session

- Adding a terminal or an agent can launch it in a new git worktree on its own branch, so parallel agents never share a working tree. It works for Herdr sessions on this Mac or an SSH host, and for local sessions, where sushiAI creates the worktree next to the repository. The new worktree joins the project's row in the sidebar.
- A worktree that belongs to an orchestrator task shows the task's title instead of its branch name; hovering shows the branch.

## Your workspace comes back after a restart

- sushiAI reopens at its last size, position and display, maximized or full screen if it was. If that display is gone or the window would be mostly off-screen, it moves onto a visible display.
- The whole workspace is kept in `workspace-state.json` in sushiAI's data folder and written a moment after every change, so quitting, a crash or a restart brings back the same layout: panes and split sizes, the selected and maximized pane, tabs or split view, the open page and the Code, Agent or Chat mode.
- Panes remember what they showed: a Files pane reopens on the same folder and file, the Orchestrator on the task you were reading, and Agent tabs and the open Chat thread come back. Unsent drafts are never saved.
- A Herdr session that ended while sushiAI was closed keeps its slot and says "Session ended" with a **Reopen** button, which starts a fresh session in the same folder (and relaunches the agent).
- The first start after updating moves your previous layout into the new file once.

## Development: restart on demand

- With `npm run dev`, editing a file under `electron/` no longer kills the app mid-work. The mascot shows "Core updated - restart?"; click **Restart** when ready, or dismiss it to keep working on the old code. Renderer edits still hot-reload.

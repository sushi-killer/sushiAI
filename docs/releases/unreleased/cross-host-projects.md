## Merged projects match reliably, stay on the Dashboard and share one canvas

- The same project on Local and on an SSH host now merges even when the two checkouts sit in folders with different names, or when one was cloned over SSH with a custom port (`ssh://git@host:10022/...`) and the other over HTTPS. The git remote and the folder inside the repository are what identify a project.
- Clicking a merged row in the sidebar opens the project like any other row, with every host's panes on the canvas, instead of only unfolding its session list. Once open, the host chips move off the row and onto its sessions: every session shows `Local` or the host's name next to its machine icon, so nothing is shown twice.
- Closing a workspace from the sidebar ends its sessions and removes it from the sidebar, but the project stays on the Dashboard, marked closed, so it can be reopened on the same host and folder with one click - or removed from the list.
- Dashboard projects merge the same way the sidebar does: one entry per project, naming every host it lives on.
- A merged project draws the panes of every host on one canvas, so a Local terminal and an agent on an SSH host can sit side by side; Tidy, drag, resize and maximize work across them.
- Adding a session to a merged project lets you choose which host it runs on.
- The New workspace dialog no longer lists Claude Code plugins to disable; plugins are switched on or off from a workspace's own controls after it exists.
- Tabs or split panels, and which pane is maximized, are now remembered per project instead of for the whole app, so one project can keep many panes side by side while another stays focused on one.

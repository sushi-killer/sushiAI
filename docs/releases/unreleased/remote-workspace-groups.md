## Remote workspaces are labeled and grouped

- The Workspaces list in the sidebar now groups by host ("This Mac" or the SSH connection's name) instead of mixing local and remote sessions in one flat list, each group collapsible with its own live status.
- Connecting to an SSH host now shows a toast with how many workspaces were already running there, so already-open remote sessions are easy to notice and reopen instead of recreating them from scratch.

Known limitations: opening a workspace from a different host's group switches the app's single active connection without confirming first, and a workspace can linger in its group for a few seconds after its host session actually ends, until the next poll catches up.

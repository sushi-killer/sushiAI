## Cross-host workspace rows merge in flat mode, and panes name their host

- With grouping off (flat list), a project that runs on this Mac and on an SSH host now shows as one row instead of two, with a small text chip per host next to the name (`This Mac`, the host's own name) and a single status dot for whichever host you're actually on. Expanding the row lists every host's sessions in one list, each with a small icon for its machine (hover for the host name); clicking a session is what actually switches you to that machine, never the row itself.
- The host-mix marker on a cross-host row that doesn't qualify to merge (different checkout name) now sits immediately after the workspace name, with a small gap, in both flat and grouped mode - it used to sit after the remote host tag.
- A terminal or agent pane belonging to a merged workspace now names its host in the pane's corner, in a matching chip to the right of the `Herdr · live stream` note. Panes of a workspace that isn't merged are unchanged.
- The remote host tag's text colour is very slightly brighter everywhere it appears, to pass accessibility contrast now that a merged row's host chips depend on it.

- Git worktrees of one repository merge into one row the same way, on one machine or across hosts. Once a row holds several worktrees, its chips name each checkout's branch (prefixed with the host's name off this Mac), a crowded row collapses to `N worktrees`, and each session in the expanded list and each pane's corner chip carries that same label. Separate clones of a repository are never merged, even with the same remote and folder name; a clone whose row shares a name with another row shows its branch as a chip instead.

Known limitations: across hosts, two checkouts only count as one project when they share a normalized git remote and the main repository folder has the same name on both. A machine that holds two separate clones of that project contributes neither to the merged row.

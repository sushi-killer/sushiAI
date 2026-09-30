# The Orchestrator runs on your SSH hosts

Pick a host at the top of the Orchestrator panel: Local or any Connections
profile. A remote host can be any macOS or Linux machine. The app installs
orchd into `~/.sushiai/bin` there: it uploads its own build when the host's OS
and CPU match, and otherwise builds orchd from the source bundled with the app
using cargo on the host (or tells you the one command that installs Rust).
When an update replaces a running orchd, the app waits for the old daemon to
exit first and reports an error instead of leaving the host half-updated.
The app starts it detached, and drives it over the existing SSH connection - no new
open ports. The daemon keeps running when you quit the app or the laptop
sleeps; reopening reconnects and shows its tasks.

The panel checks each host for git and the claude and codex CLIs (installed
and logged in) and marks routes whose harness is missing as unavailable
there. Task rows name their host once more than one host is in use. Tasks from
every connected host count in the Dock badge, the tray, the Inbox and notices,
and opening one selects its host. orchd also builds and tests on Linux.

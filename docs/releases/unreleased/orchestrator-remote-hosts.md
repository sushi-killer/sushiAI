# The Orchestrator runs on your SSH hosts

Pick a host at the top of the Orchestrator panel: Local or any Connections
profile. On a remote host the app installs orchd into `~/.sushiai/bin`. The
packaged app uploads its own build, so in this release a remote host has to
be an Apple silicon Mac; running from a source checkout also builds orchd on
any other host with cargo (or tells you the one command that installs Rust).
The app starts it detached, and drives it over the existing SSH connection - no new
open ports. The daemon keeps running when you quit the app or the laptop
sleeps; reopening reconnects and shows its tasks.

The panel checks each host for git and the claude and codex CLIs (installed
and logged in) and marks routes whose harness is missing as unavailable
there. Task rows name their host once more than one host is in use. Tasks from
every connected host count in the Dock badge, the tray, the Inbox and notices,
and opening one selects its host. orchd also builds and tests on Linux.

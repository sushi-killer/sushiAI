## The orchestrator runs inside the sushiai daemon

- The orchestrator is now a module of the `sushiai` daemon. There is no separate `orchd` process, socket or token: the app talks to the orchestrator over the same daemon connection as sessions, including on SSH hosts.
- The orchestrator is off by default. Turn it on in **Extensions**; this registers its MCP server and restarts the daemon, and sessions keep running. A setup that already used `orchd` keeps it on, and an earlier "off" is never overridden.
- Orchestrator runs that write files keep going when the daemon restarts, and are picked up again afterwards.
- A legacy `orchd` that is still running is stopped the first time the new daemon starts. **Install sushiai** on a host also removes its old `orchd` binary.
- Breaking: orchestrator data starts clean. Tasks from the old `orchd` data folder are not moved; the old folder stays untouched as an archive.
- The packaged app no longer ships the `orchd` source archive.

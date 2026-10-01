Persistent Herdr sessions now use endpoint-specific identities and host-canonical checkout paths. Saved layouts and panel selection migrate while preserving live sessions, conversations, local panels, and separate worktrees.

Project creation, additional panels, and retries share an intent-aware operation queue and a durable creation journal. Preparation failures remain visible and retry the created session. Environment variables reach new terminal processes through Herdr's native launch environment, including worktrees and SSH.

Native Herdr events trigger coordinated snapshots with reconnect recovery and a one-minute reconciliation interval. Connections settings checks daemon and terminal CLI compatibility separately and installs a pinned, checksum-verified CLI. Terminal delivery uses bounded acknowledged queues and reports corrupted or disconnected streams. Measurements and validation are recorded in `docs/HERDR-STABILITY.md`.

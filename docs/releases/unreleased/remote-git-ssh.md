## Remote repository access

- Preparing a remote host retries an unavailable HTTPS clone over SSH. Authentication, server trust, network, branch, destination and setup failures show separate recovery guidance.
- Get and copy the remote host's public SSH key from preparation, add it to GitLab or GitHub, and retry without signing in to the host manually. The private key stays on the host and repeat requests reuse it.
- Review Git server fingerprints and explicitly trust a new or replacement key from the same window. Existing SSH settings remain intact; sushiAI keeps its own trust records.
- Adjust the SSH clone URL and port during recovery. Session starts, orchestrator runs, project Hosts and New project use the same recovery controls, and local repository inspection failures no longer block remote setup.
- Failed clone attempts clean up only their own temporary checkout. Existing project files and unfinished folders are preserved.

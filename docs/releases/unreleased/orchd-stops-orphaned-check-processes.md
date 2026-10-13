## Orchestrator

- A finished check no longer leaves behind the processes it started in their own session, such as a daemon that a test run spawned. Before, they lived on after the check ended and piled up on a long-running host.

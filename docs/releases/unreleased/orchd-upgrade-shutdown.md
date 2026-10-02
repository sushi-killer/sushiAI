## Orchestrator

- Updating the orchestrator on an SSH host no longer fails with "did not stop within 10 s" when the connection is slow to come up: sushiAI now retries until the running orchestrator receives the shutdown, instead of giving up after one early attempt.

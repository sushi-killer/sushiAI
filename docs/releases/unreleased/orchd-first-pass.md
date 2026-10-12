orchd records first-pass metrics for every finished task and plans backlog requests when they start.

- A task that ends `done` or `failed` gets `metrics`: whether it passed on the first implement attempt, how often the owner was needed, spend per phase and, for a failed task, the cause (`env`, `plan`, `impl` or `review_loop`). A top-level task adds one `done` entry to the overseer log with that line and the repo's first-pass rate and average cost per landed task over its last 20 finished tasks.
- A backlog task created from a request stays `queued` and is planned only when it starts, on the head of its base branch as it is then. Backlog tasks created before this change keep their old behaviour.

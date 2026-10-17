## Orchestrator tasks start after an owner answer

- A task no longer stays queued after the owner answers a question. A check decision written while the first attempt started could overwrite the attempt, and the task then never ran on.

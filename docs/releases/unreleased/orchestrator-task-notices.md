## Orchestrator: mascot notices for finished, failed and waiting tasks

- A top-level task that finishes, fails, or needs your input raises one mascot
  notice: an in-app toast while the window is focused, the native mascot
  notification otherwise. A finished task with a fresh report reads
  "Feature done: <title>" with its cost and landing; a failed one names the
  failure. Needs-input toasts stay until you act.
- Clicking a notice opens the Orchestrator page at that task: its question,
  its Report section (done) or its attempts (failed). Subtasks raise only
  their questions, and archived tasks raise nothing.

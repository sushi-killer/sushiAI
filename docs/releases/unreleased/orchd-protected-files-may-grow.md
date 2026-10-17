## Orchestrator keeps new cases in a test file a check names

- An agent may now add new lines, such as new test cases, to an existing test file that a task's checks name; before, orchd put the file back and the task burned its attempts. A changed or removed line is still put back, and the planner's own test files stay unchangeable. The review is shown the added lines and asked to confirm they do not weaken a test.

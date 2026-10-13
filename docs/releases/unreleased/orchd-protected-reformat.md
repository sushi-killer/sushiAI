## Orchestrator

- A planner test that an attempt only reformats (whitespace outside quotes, for example by a newer rustfmt or prettier) no longer counts as a change to a protected file, so the task does not stop on a formatter. Files where whitespace counts, such as Python, YAML and Makefiles, are still checked byte for byte.

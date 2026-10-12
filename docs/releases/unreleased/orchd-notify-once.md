## Orchestrator

- The orchestrator agent now tells you about each decision once. A reply reaches your chat only when a question it was woken for is still open; a turn that settled the question itself (a retry, a base check) is logged as done and says nothing. A question you already heard about is not repeated, also after a restart, until `waitRemindEveryHours` has passed.
- A notice is at most three lines: the decision, the options and a recommendation. A restart wake re-reads the waiting questions and decides silently.
- New `quietHours` setting (default `23:00-08:00` local time, `""` = off): in that window the agent starts no reminder and holds a notice for the morning digest, or, with `digestHour` off, for the first check after the window ends.

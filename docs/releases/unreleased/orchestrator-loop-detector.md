## Orchestrator: loop detector

- orchd stops an implement attempt that repeats itself (the same tool call three times in a row, three error results in a row, or eight edits of one file with no command run in between) and records a `loop` failure naming the repeated call or file. The next attempt's brief says so, and a second loop on a task moves it up a tier. Turn it off per task with the `loopDetect` variant flag.

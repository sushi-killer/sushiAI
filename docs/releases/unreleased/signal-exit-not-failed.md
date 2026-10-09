## Sessions

- An agent started as the pane command and then closed with a signal now exits with 128 plus the signal number, like a shell reports it, instead of a bare 1. Clients can tell a closed agent (terminate, kill, hangup, interrupt) from a real crash.

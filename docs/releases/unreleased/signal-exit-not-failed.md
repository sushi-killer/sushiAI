## Sessions

- An agent started as the pane command and then closed with a signal (terminate, kill, hangup, interrupt) now shows as stopped, not failed. The daemon reports a signal exit as 128 plus the signal number, like a shell does. A real crash signal still shows as failed.

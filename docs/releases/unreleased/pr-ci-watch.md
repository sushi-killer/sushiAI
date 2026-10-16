## Pull request checks are watched

- After orchd opens a pull request it watches the pull request's GitHub checks until they finish.
- A red check that is the task's own goes back to the task's agent for one repair step with the failed job log, and the fix is pushed to the same pull request.
- A red check that is not the task's (it passes on a rerun of the failed jobs, or the base branch fails the same test) leaves the task alone, and the owner gets one note naming the test and why.

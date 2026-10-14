## A pull request carry that conflicts no longer ends without a pull request

- When the finished work conflicts with the newest base while orchd carries it for the pull request, the task goes back for one conflict-resolution attempt on the new base. The checks and the review run again, then the pull request opens. If the attempt or the review fails, or the work still conflicts, the task ends `failed` with a report and one notice.
- When the newest base already holds the work, orchd records that and opens no pull request.
- `task.pr` carries the branch onto the newest base first, as the automatic pull request does.

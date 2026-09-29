## orchd records who asked each task question

- Every task question now carries `askedBy` (the stage that asked it: `brief`, `plan`, `implement`, `verify`, `review`, `advisor` or `land`) and `askedAt` (a millisecond timestamp). Tasks saved before this change load unchanged, and their questions simply have neither key.
- Each task keeps a `questionHistory` of every question that was answered, oldest first. An entry has the question text, its options and kind, `askedBy` and `askedAt`, the `answer`, `answeredAt`, and `answeredBy` (`owner`, `policy`, `judge` or `orchestrator`). Questions that were cleared without an answer, such as a task stop or a dependency question that went moot, are not recorded. A task with no answered questions has no `questionHistory` key.
- Both fields appear wherever the full task is sent: the `task` event, `task.get` and `task.list`. No request parameter, existing field, `decisions` or `assumptions` changed.

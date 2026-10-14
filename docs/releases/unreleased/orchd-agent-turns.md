## orchd review and failure checks name things by id, not by text

- A tool of a connected MCP server that declares no read-only hint and has no owner override now counts as a write and asks the owner first. The tool name no longer decides.
- The reviewer rules on each acceptance criterion by its number and reports each finding with an `id` (a registered finding or `null`) and `causedByDiff`. A criterion text that starts like another criterion can no longer settle the wrong one, a new finding worded like a resolved one is not dropped, and a problem the diff causes in a file the task did not touch now blocks after the first attempt.
- A disputed review finding is named by its id (`F-1`) in the implementer's `disputes`.
- The red-base waiver compares the full passing and failing test lists, so it also works for suites of more than 500 tests.
- A failing test with `timeout` in its name no longer counts as an interrupted check.
- Any non-test change since the earlier screenshots reruns the screenshot command, not only a change to a UI file.

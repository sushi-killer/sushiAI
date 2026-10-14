orchd no longer treats a failing check as the task's own target because a test name appears in the request, so a flaky shared check is handled as the environment.

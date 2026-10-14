## Base failures are attributed by an agent, with evidence code checks

- When a check fails on a task and on its base and the two runs cannot be compared by test name, one read-only agent turn now says whose failure it is: introduced by the task, inherited from the base, the environment, or incomplete. Before, orchd asked the owner.
- A verdict other than "introduced" holds only with evidence. An "inherited" verdict is accepted only when every test that fails on the task also fails on the base, and every name it gives is a failing test or a whole line of the base output; otherwise the failure is the task's. A line in a run that claims a failure is old counts for nothing.
- An "environment" or "incomplete" verdict first reruns the check once on the task tree. Only a verdict that still stands after the rerun asks the owner.
- A waiver for an inherited failure is an assumption the owner can overturn, and the pull request body lists it under "Waived base failures". A waiver never lands work by itself.
- The environment is blamed only on a signal orchd sees itself (a failed setup or a timeout), never on the agent's word. A failed top-level task whose last attempt failed that way is restarted once by orchd. A second failure is reported to the owner as before.
- The check "is this the task's target" no longer reads names or paths out of the check output.

## orchd: briefs that heal themselves

- Before the first attempt orchd runs every `verify`, `finalVerify` and check
  command on the base. A command the repository cannot run (no test runner or
  harness for that kind of check) is rewritten to the nearest check it can run,
  with its criterion; one that fails on the base for a reason unrelated to the
  task stops gating. Both are decision lines and assumptions you can overturn.
- When review marks the same criterion unmet in two attempts, a cheap judge run
  (`briefCheckRoute`) decides between a code gap (retry as before) and a
  criterion that is infeasible as written (amended, recorded, and the same
  attempt is reviewed again without costing another attempt).
- The planner's `screenshot` command is run by orchd itself after verify, so an
  attempt is never failed for a missing image. Without one, images from an
  earlier attempt still count while no UI file that attempt changed has changed;
  the reviewer sees each image labelled with its attempt.
- After the first attempt only P0/P1 findings about the task's own work cost an
  attempt; P2 and unrelated findings become follow-ups in the task report.
- An owner or policy answer that contradicts a criterion amends that criterion
  before the next attempt and review, with a decision line.

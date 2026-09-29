## orchd: answer policy

- Routine task questions are answered by fixed rules and a cheap judge, behind
  `answerPolicy` (on by default; `off` sends every question to the owner as
  before). Every question now has a `kind`. A review with no verdict twice is reviewed
  again (once per attempt, the same attempt, never a new implement attempt), failing attempts continue once, an ended dependency is
  retried once, a check that already fails on the base waits for the base to
  move once, and a plan or agent question with an option phrased as the
  cautious or reversible one takes it when triage agrees. An "impossible
  criterion" claim goes to a read-only judge run on `briefCheckRoute`, which
  drops the criterion only when it can verify the evidence and otherwise
  retries. Budget and protected-path questions always reach the owner, and so
  does every question after the third automatic answer on a task. Each
  automatic answer is recorded as an assumption (`by` is `policy` or `judge`)
  the owner can overturn, with a `Policy:` decision line.

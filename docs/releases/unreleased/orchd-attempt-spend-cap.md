## orchd: per-attempt spend cap and pinned effort

- `maxAttemptCostUsd` (0 = off, the default) caps what one implement attempt
  may spend. Claude runs also get it as `--max-budget-usd`; orchd itself
  stops the run once its streamed usage, priced with `settings.prices`,
  passes the cap (Codex included). The attempt fails with the new kind
  `budget` and the task retries as usual, or asks the owner first when the
  task budget (`maxCostUsd`) is also spent.
- Every Claude and Codex run now passes its route's effort explicitly, even
  when it equals the CLI default, so a CLI default change cannot move
  results. A route with no effort still passes none.

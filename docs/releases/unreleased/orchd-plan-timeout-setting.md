## The planner time limit is a setting

- `planTimeoutSecs` sets how long the orchd planner may run, 30 minutes by default (it was 15). The one resumed retry after a timeout still gets twice the time.

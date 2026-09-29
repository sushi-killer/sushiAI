## orchd: feature report and leadTouch

- A finished top-level task or graph writes one short report (no model call)
  to `<data>/tasks/<id>/report.md` and `task.report`: outcome, what changed,
  each criterion's status, assumptions and automatic answers, cost by stage
  and model, attempts and failure kinds, follow-ups from the handoffs. It
  is announced by the one mascot notice, titled "Feature done: <title>"
  (opening it scrolls to the Report section), and shows as a Report section at
  the top of the task's detail. `task.report {id}` and the `task_report`
  orchestrator tool return it.
- `leadTouch` marks whether a person had to fix a done task's work
  (`task.leadTouch`, the `task_lead_touch` tool, a "Needed a fix" / "Clean"
  toggle in the panel). A landing that was rewritten, or a non-orchd commit on
  its files within 24h, marks it touched automatically. `orchd costs` and
  `costs.summary` report the rate per repo and per week.

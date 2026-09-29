## orchd: evidence is only what visual criteria name

- An attempt's evidence is now the `artifacts/<file>` images its visual
  criteria name, plus images under a directory a criterion's check names.
  Other images the attempt wrote (the desktop smoke's `workspace.png`,
  `smoke-hidden-window.png`, `failure.png`) are not saved as evidence, not
  sent to the reviewer and do not satisfy the evidence gate, which now names
  the missing files. A visual criterion that names no path keeps the old
  behaviour for the whole attempt; a task with no visual criterion saves no
  evidence. Reused earlier-attempt copies are filtered the same way. The plan
  brief asks for visual criteria only on changed screens, with one named
  screenshot file per screen.

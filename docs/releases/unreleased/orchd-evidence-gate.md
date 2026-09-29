## orchd: evidence gate

- A task with a visual criterion (its `-- check:` names a screenshot or an
  image under `artifacts/`, or the planner wrote it as `{text, visual: true}`)
  now needs an image saved under `artifacts/` during the attempt before
  review; otherwise the attempt fails with the new failure kind `evidence`.
  Every image an attempt saves is copied to `runs/<n>/evidence/`, listed by
  `task.get` and shown as thumbnails in the task detail.

## Orchestrator settings show what differs from the defaults

- Settings → Orchestration marks every setting whose saved value differs from orchd's built-in default with "Default: ..." beside the control, naming the default route by its label. Settings saved before a default changed no longer stay on the old value unnoticed: an owner who saved while the planner defaulted to Claude Sonnet now sees that the default is Claude Opus.
- A reset button next to each marker puts that one setting back to its default. Like any other edit in the panel, it is only applied when you press **Save**.
- When the default route has been removed from your routes, the marker stays but its reset is disabled, so a setting can never point at a route that does not exist.
- The route list and the classifier's API-key provider have no defaults to compare against and are never marked. With an older orchd that cannot report its defaults, the panel works as before, without markers.

## orchd plans with experience from its own records

- Every plan brief now carries a **Past work** section: up to three finished tasks of the same repo that changed the files the request names (or, when no file matches, talk about the same things), with their outcome, implement attempts, failure kinds, the verify commands that failed and the follow-ups they left. It is capped at 1,500 characters and left out for eval tasks and when there is nothing to show.
- Repo notes are the owner's standing guidance for a repository, kept in `repo-notes.json` beside the other orchd data and shown first in the Past work section. They are managed with `repo.notes.list`, `repo.notes.add` and `repo.notes.remove`, and the orchestrator agent has matching tools, which it uses only when you ask.
- Approving a doc or command evolution proposal also saves its change as a repo note.
- The Orchestrator panel has a Repo notes section next to the proposals, where notes can be listed, added and removed; notes saved from an approved proposal are marked.
